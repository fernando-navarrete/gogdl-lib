use std::io::Read;

use flate2::read::ZlibDecoder;
use futures::StreamExt;
use reqwest::Client;
use serde::de::DeserializeOwned;
use tokio::sync::mpsc;
use url::Url;

use crate::client::error::ClientError;

#[derive(Clone)]
pub struct HttpClient {
    client: Client,
}

impl HttpClient {
    pub fn new_with_client(client: Client) -> Self {
        Self { client }
    }
    pub async fn get_json<T: DeserializeOwned>(&self, url: &str) -> Result<T, ClientError> {
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
    pub fn fetch_chunk_stream(
        &self,
        url: &str,
        auth: String,
    ) -> mpsc::UnboundedReceiver<Result<Vec<u8>, ClientError>> {
        let (tx, rx) = mpsc::unbounded_channel();
        let url = url.to_string();
        let auth = auth;

        let client = self.client.clone();
        tokio::spawn(async move {
            let result: Result<Vec<u8>, ClientError> = async {
                let url = Url::parse(&url)?;
                let mut request = client.get(url);
                if !auth.is_empty() {
                    request = request.bearer_auth(&auth);
                }
                let response = request.send().await?;

                let status = response.status();
                if !status.is_success() {
                    let response_text = response.text().await?;
                    return Err(ClientError::Http {
                        status,
                        body: response_text,
                    });
                }

                let mut stream = response.bytes_stream();
                let mut buffer: Vec<u8> = Vec::new();
                while let Some(chunk) = stream.next().await {
                    match chunk {
                        Ok(downloaded_bytes) => buffer.extend_from_slice(&downloaded_bytes),
                        Err(_err) => return Err(ClientError::StreamError()),
                    }
                }

                let mut decoded_buffer = Vec::new();
                let mut z = ZlibDecoder::new(&buffer[..]);
                z.read_to_end(&mut decoded_buffer)?;
                Ok(decoded_buffer)
            }
            .await;

            tx.send(result).ok();
        });

        rx
    }
    pub async fn get_and_decode<T: DeserializeOwned>(
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
    pub async fn get_json_with_auth<T: DeserializeOwned>(
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
