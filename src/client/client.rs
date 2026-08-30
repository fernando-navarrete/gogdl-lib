use std::io::Read;

use bytes::Bytes;
use flate2::read::ZlibDecoder;
use futures_util::StreamExt;
use reqwest::Client;
use serde::de::DeserializeOwned;

use crate::client::{ClientError::AuthError, Request, error::ClientError};

#[derive(Clone)]
pub struct HttpClient {
    client: Client,
}

impl HttpClient {
    pub fn new_with_client(client: Client) -> Self {
        Self { client }
    }
    pub async fn fetch<T: DeserializeOwned>(&self, request: Request) -> Result<T, ClientError> {
        match request {
            Request::AuthDecode { url, auth_manager } => {
                let auth = match auth_manager.get_auth().await {
                    Ok(auth) => auth,
                    Err(err) => return Err(AuthError(err)),
                };
                let result = self.get_and_decode(&url, &auth.access_token).await?;
                Ok(result)
            }
            Request::Get { url } => {
                let result = self.get_json(&url).await?;
                Ok(result)
            }
            Request::GetAuth { url, auth_manager } => {
                let auth = match auth_manager.get_auth().await {
                    Ok(auth) => auth,
                    Err(err) => return Err(AuthError(err)),
                };
                let result = self.get_json_with_auth(&url, &auth.access_token).await?;
                Ok(result)
            }
        }
    }
    async fn get_json<T: DeserializeOwned>(&self, url: &str) -> Result<T, ClientError> {
        let url = reqwest::Url::parse(url)?;
        let request = self.client.get(url);
        let response = request.send().await?;

        if !response.status().is_success() {
            let response_status = response.status();
            let response_text = response.text().await?;
            return Err(ClientError::Http {
                status: response_status,
                body: response_text,
            });
        }
        let response_text = response.text().await?;
        let result: T = serde_json::from_str(&response_text)?;
        Ok(result)
    }
    pub async fn stream_chunk(
        &self,
        url: &str,
        mut f: impl FnMut(Bytes) -> std::io::Result<()>,
    ) -> Result<(), ClientError> {
        let url = reqwest::Url::parse(url)?;
        let request = self.client.get(url);

        let response = request.send().await?;

        if !response.status().is_success() {
            let response_status = response.status();
            let response_text = response.text().await?;
            return Err(ClientError::Http {
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
            f(chunk)?;
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
            return Err(ClientError::Http {
                status: response_status,
                body: response_text,
            });
        }

        let response_bytes = response.bytes().await?;
        let mut z = ZlibDecoder::new(&response_bytes[..]);
        let mut s = String::new();
        z.read_to_string(&mut s)?;
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
            return Err(ClientError::Http {
                status: response_status,
                body: response_text,
            });
        }
        let response_text = response.text().await?;
        let result: T = serde_json::from_str(&response_text)?;
        Ok(result)
    }
}
