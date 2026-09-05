use std::{io::Read, sync::Arc};

use bytes::Bytes;
use flate2::read::ZlibDecoder;
use futures_util::StreamExt;
use reqwest::Client;
use serde::de::DeserializeOwned;

use crate::{
    client::{
        TokenObserver,
        auth::{AuthError, AuthManager},
        error::ClientError,
    },
    downloader::FileType,
    secure_links::SecureLinksManager,
};

#[derive(Clone)]
pub struct HttpClient {
    client: Client,
    auth_manager: AuthManager,
}

impl HttpClient {
    pub fn new_with_client(client: Client) -> Self {
        Self {
            client,
            auth_manager: AuthManager::new(),
        }
    }
    pub async fn fetch_no_retry<T: DeserializeOwned>(
        &self,
        url: &str,
        decode: bool,
        require_auth: bool,
    ) -> Result<T, ClientError> {
        self.inner_fetch(url, decode, require_auth).await
    }
    pub async fn fetch<T: DeserializeOwned>(
        &self,
        url: &str,
        decode: bool,
        require_auth: bool,
    ) -> Result<T, ClientError> {
        let mut attempts = 0;
        while attempts < 3 {
            attempts += 1;
            match self.inner_fetch(url, decode, require_auth).await {
                Ok(result) => return Ok(result),
                Err(ClientError::HttpError { status, body }) => {
                    let _ = body;
                    if status == reqwest::StatusCode::UNAUTHORIZED {
                        if require_auth {
                            self.auth_manager.refresh_auth(&self).await?;
                        }
                    } else {
                        return Err(ClientError::HttpError { status, body: body });
                    }
                }
                Err(ClientError::AuthError(AuthError::TokenExpired)) => {
                    self.auth_manager.refresh_auth(&self).await?;
                }
                Err(err) => {
                    return Err(err);
                }
            }
        }
        Err(ClientError::MaxRetriesReached)
    }
    pub async fn stream_chunk(
        &self,
        secure_links_manager: &SecureLinksManager,
        game_id: &str,
        file_type: &FileType,
        chunk_hash: &str,
        mut f: impl AsyncFnMut(Bytes) -> std::io::Result<()>,
    ) -> Result<(), ClientError> {
        let links = match secure_links_manager.get_secure_links(game_id).await {
            Ok(links) => links,
            Err(err) => {
                return Err(ClientError::SecureLinksError {
                    inner: err.to_string(),
                });
            }
        };

        let url_format = match links.get_highest_priority_url() {
            Ok(url_format) => url_format,
            Err(err) => {
                return Err(ClientError::SecureLinksError {
                    inner: err.to_string(),
                });
            }
        };

        let url = match file_type {
            FileType::DepotFile => url_format.parse_url(chunk_hash),
            FileType::Other => url_format.parse_url_redist(chunk_hash),
        };

        self.stream_chunk_inner(&url, &mut f).await
    }
    pub async fn restore_auth_from_string(&self, json_str: &str) -> Result<(), ClientError> {
        self.auth_manager.restore_from_string(json_str).await?;
        Ok(())
    }
    pub fn get_login_url(&self) -> &str {
        self.auth_manager.get_login_url()
    }
    pub async fn login_with_code(&self, code: &str) -> Result<String, ClientError> {
        let token = self.auth_manager.login_with_code(code, self).await?;
        Ok(token)
    }
    pub async fn set_token_observer(&self, observer: Arc<dyn TokenObserver>) {
        self.auth_manager.set_token_observer(observer).await;
    }
    pub async fn remove_token_observer(&self) {
        self.auth_manager.remove_token_observer().await;
    }
    async fn inner_fetch<T: DeserializeOwned>(
        &self,
        url: &str,
        decode: bool,
        require_auth: bool,
    ) -> Result<T, ClientError> {
        if require_auth {
            let auth = match self.auth_manager.get_auth().await {
                Ok(auth) => auth,
                Err(err) => return Err(ClientError::AuthError(err)),
            };
            if decode {
                let result = self.get_and_decode(url, &auth.access_token).await?;
                return Ok(result);
            }
            let result = self.get_json_with_auth(url, &auth.access_token).await?;
            return Ok(result);
        } else {
            let result = self.get_json(url).await?;
            return Ok(result);
        }
    }
    async fn get_json<T: DeserializeOwned>(&self, url: &str) -> Result<T, ClientError> {
        let url = reqwest::Url::parse(url)?;
        let request = self.client.get(url);
        let response = request.send().await?;

        if !response.status().is_success() {
            let response_status = response.status();
            let response_text = response.text().await?;
            return Err(ClientError::HttpError {
                status: response_status,
                body: response_text,
            });
        }
        let response_text = response.text().await?;
        let result: T = serde_json::from_str(&response_text)?;
        Ok(result)
    }
    pub async fn stream_chunk_inner(
        &self,
        url: &str,
        f: &mut impl AsyncFnMut(Bytes) -> std::io::Result<()>,
    ) -> Result<(), ClientError> {
        let url = reqwest::Url::parse(url)?;
        let request = self.client.get(url);

        let response = request.send().await?;

        if !response.status().is_success() {
            let response_status = response.status();
            let response_text = response.text().await?;
            return Err(ClientError::HttpError {
                status: response_status,
                body: response_text,
            });
        }

        let mut stream = response.bytes_stream();

        while let Some(chunk) = stream.next().await {
            let chunk = match chunk {
                Ok(chunk) => chunk,
                Err(e) => return Err(ClientError::NetworkError(e)),
            };
            if let Err(e) = f(chunk).await {
                return Err(ClientError::ChunkStreamCallbackError(e));
            }
        }
        Ok(())
    }
    async fn get_and_decode<T: DeserializeOwned>(
        &self,
        url: &str,
        auth_token: &str,
    ) -> Result<T, ClientError> {
        let url = reqwest::Url::parse(url)?;
        let mut request = self.client.get(url);
        request = request.bearer_auth(auth_token);

        let response = request.send().await?;

        if !response.status().is_success() {
            let response_status = response.status();
            let response_text = response.text().await?;
            return Err(ClientError::HttpError {
                status: response_status,
                body: response_text,
            });
        }

        let response_bytes = response.bytes().await?;
        let mut z = ZlibDecoder::new(&response_bytes[..]);
        let mut s = String::new();

        if let Err(e) = z.read_to_string(&mut s) {
            return Err(ClientError::DecodeError(e));
        }

        let data: T = serde_json::from_str(&s)?;

        Ok(data)
    }
    async fn get_json_with_auth<T: DeserializeOwned>(
        &self,
        url: &str,
        auth_token: &str,
    ) -> Result<T, ClientError> {
        let url = reqwest::Url::parse(url)?;
        let mut request = self.client.get(url);
        request = request.bearer_auth(auth_token);

        let response = request.send().await?;

        if !response.status().is_success() {
            let response_status = response.status();
            let response_text = response.text().await?;
            return Err(ClientError::HttpError {
                status: response_status,
                body: response_text,
            });
        }
        let response_text = response.text().await?;
        let result: T = serde_json::from_str(&response_text)?;
        Ok(result)
    }
}
