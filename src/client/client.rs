use std::{io::Read, sync::Arc};

use bytes::Bytes;
use flate2::read::ZlibDecoder;
use futures_util::{StreamExt, future::BoxFuture};
use reqwest::Client;
use reqwest::header::HeaderMap;
use serde::de::DeserializeOwned;

use crate::{
    client::{
        TokenObserver,
        auth::{AuthError, AuthManager},
        error::ClientError,
    },
    constants::MAX_ATTEMPTS,
    downloader::backoff,
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
        headers: Option<&[(&str, &str)]>,
    ) -> Result<T, ClientError> {
        self.inner_fetch(url, decode, require_auth, headers).await
    }
    pub async fn fetch<T: DeserializeOwned>(
        &self,
        url: &str,
        decode: bool,
        require_auth: bool,
        headers: Option<&[(&str, &str)]>,
    ) -> Result<T, ClientError> {
        for attempt in 0..MAX_ATTEMPTS {
            match self.inner_fetch(url, decode, require_auth, headers).await {
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
                Err(ClientError::NetworkError(err)) => {
                    if attempt != MAX_ATTEMPTS - 1 {
                        backoff(attempt as u32).await;
                        continue;
                    }
                    return Err(ClientError::NetworkError(err));
                }
                Err(err) => {
                    return Err(err);
                }
            }
        }
        Err(ClientError::MaxRetriesReached)
    }
    pub async fn stream_chunk<'a>(
        &'a self,
        url: &str,
        mut f: impl FnMut(Bytes) -> BoxFuture<'a, std::io::Result<()>>,
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
    /// Like [`stream_chunk`](Self::stream_chunk), but sends `headers` with the
    /// request and hands back the response headers once the body has been
    /// fully streamed. Not retried.
    pub async fn stream_chunk_with_headers<'a>(
        &'a self,
        url: &str,
        headers: Option<&[(&str, &str)]>,
        mut f: impl FnMut(Bytes) -> BoxFuture<'a, std::io::Result<()>>,
    ) -> Result<HeaderMap, ClientError> {
        let url = reqwest::Url::parse(url)?;
        let mut request = self.client.get(url);
        if let Some(headers) = headers {
            for (key, value) in headers {
                request = request.header(*key, *value);
            }
        }

        let response = request.send().await?;

        if !response.status().is_success() {
            let response_status = response.status();
            let response_text = response.text().await?;
            return Err(ClientError::HttpError {
                status: response_status,
                body: response_text,
            });
        }

        let response_headers = response.headers().clone();
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
        Ok(response_headers)
    }
    /// Sends `body` as a PUT request, in 16 KiB chunks, calling `on_progress`
    /// with each chunk's length as it is handed to the transport (not when
    /// the server acknowledges it). Not retried.
    pub async fn put_stream(
        &self,
        url: &str,
        headers: Option<&[(&str, &str)]>,
        body: Vec<u8>,
        mut on_progress: impl FnMut(usize) + Send + 'static,
    ) -> Result<(), ClientError> {
        const CHUNK_SIZE: usize = 16 * 1024;

        let url = reqwest::Url::parse(url)?;
        let mut request = self.client.put(url).header("Content-Length", body.len());
        if let Some(headers) = headers {
            for (key, value) in headers {
                request = request.header(*key, *value);
            }
        }

        let mut remaining = Bytes::from(body);
        let chunks = futures_util::stream::iter(std::iter::from_fn(move || {
            if remaining.is_empty() {
                return None;
            }
            let chunk = remaining.split_to(remaining.len().min(CHUNK_SIZE));
            on_progress(chunk.len());
            Some(Ok::<_, std::io::Error>(chunk))
        }));

        let response = request
            .body(reqwest::Body::wrap_stream(chunks))
            .send()
            .await?;

        if !response.status().is_success() {
            let response_status = response.status();
            let response_text = response.text().await?;
            return Err(ClientError::HttpError {
                status: response_status,
                body: response_text,
            });
        }
        Ok(())
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
        headers: Option<&[(&str, &str)]>,
    ) -> Result<T, ClientError> {
        if require_auth {
            let auth = match self.auth_manager.get_auth().await {
                Ok(auth) => auth,
                Err(err) => return Err(ClientError::AuthError(err)),
            };
            if decode {
                let result = self
                    .get_and_decode(url, &auth.access_token, headers)
                    .await?;
                return Ok(result);
            }
            let result = self
                .get_json_with_auth(url, &auth.access_token, headers)
                .await?;
            return Ok(result);
        } else {
            let result = self.get_json(url, headers).await?;
            return Ok(result);
        }
    }
    async fn get_json<T: DeserializeOwned>(
        &self,
        url: &str,
        headers: Option<&[(&str, &str)]>,
    ) -> Result<T, ClientError> {
        let url = reqwest::Url::parse(url)?;
        let mut request = self.client.get(url);
        if let Some(headers) = headers {
            for (key, value) in headers {
                request = request.header(*key, *value);
            }
        }
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
    async fn get_and_decode<T: DeserializeOwned>(
        &self,
        url: &str,
        auth_token: &str,
        headers: Option<&[(&str, &str)]>,
    ) -> Result<T, ClientError> {
        let url = reqwest::Url::parse(url)?;
        let mut request = self.client.get(url);
        request = request.bearer_auth(auth_token);
        if let Some(headers) = headers {
            for (key, value) in headers {
                request = request.header(*key, *value);
            }
        }

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
        headers: Option<&[(&str, &str)]>,
    ) -> Result<T, ClientError> {
        let url = reqwest::Url::parse(url)?;
        let mut request = self.client.get(url);
        request = request.bearer_auth(auth_token);
        if let Some(headers) = headers {
            for (key, value) in headers {
                request = request.header(*key, *value);
            }
        }

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
    /// Returns the current session's refresh token, without refreshing.
    ///
    /// # Errors
    /// [`ClientError::AuthError`] wrapping [`AuthError::NotAuthenticated`] if
    /// no session is logged in, or [`AuthError::TokenExpired`] if the access
    /// token has expired — even though the refresh token itself is likely
    /// still usable.
    pub async fn get_refresh_token(&self) -> Result<String, ClientError> {
        let auth = self.auth_manager.get_auth().await?;
        Ok(auth.refresh_token)
    }
}
