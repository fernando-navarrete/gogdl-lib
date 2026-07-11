use std::io::{Read, Write};

use flate2::read::ZlibDecoder;
use flate2::write::ZlibDecoder as ZlibStreamDecoder;
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
    /// Shared GET path for every non-streaming request below: parses
    /// `url`, attaches a bearer token if one is given, sends the request,
    /// and turns a non-2xx response into `ClientError::Http`. Callers
    /// decode the successful response body however they need (plain text
    /// vs. zlib-compressed bytes).
    async fn send_get(
        &self,
        url: &str,
        auth_token: Option<&str>,
    ) -> Result<reqwest::Response, ClientError> {
        let url = reqwest::Url::parse(url)?;
        let mut request = self.client.get(url);
        if let Some(token) = auth_token {
            request = request.bearer_auth(token);
        }
        let response = request.send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await?;
            return Err(ClientError::Http { status, body });
        }
        Ok(response)
    }
    pub async fn get_json<T: DeserializeOwned>(&self, url: &str) -> Result<T, ClientError> {
        let response = self.send_get(url, None).await?;
        let response_text = response.text().await?;
        let result: T = serde_json::from_str(&response_text)?;
        Ok(result)
    }
    /// Streams and decodes a chunk incrementally: as each network-level piece
    /// of the (zlib-compressed) response body arrives, it is fed through a
    /// streaming decoder and the decoded output produced so far is sent
    /// immediately, so neither side needs to hold the whole chunk in memory
    /// at once. The receiver may get zero or more `Ok(bytes)` messages
    /// followed by channel closure (success) or an `Err(...)` (failure).
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
            let parsed_url = match Url::parse(&url) {
                Ok(url) => url,
                Err(e) => {
                    tx.send(Err(e.into())).ok();
                    return;
                }
            };
            let mut request = client.get(parsed_url);
            if !auth.is_empty() {
                request = request.bearer_auth(&auth);
            }

            let response = match request.send().await {
                Ok(response) => response,
                Err(e) => {
                    tx.send(Err(e.into())).ok();
                    return;
                }
            };

            let status = response.status();
            if !status.is_success() {
                let response_text = response.text().await.unwrap_or_default();
                tx.send(Err(ClientError::Http {
                    status,
                    body: response_text,
                }))
                .ok();
                return;
            }

            let mut stream = response.bytes_stream();
            let mut decoder = ZlibStreamDecoder::new(Vec::new());
            while let Some(chunk) = stream.next().await {
                let downloaded_bytes = match chunk {
                    Ok(bytes) => bytes,
                    Err(_err) => {
                        tx.send(Err(ClientError::StreamError())).ok();
                        return;
                    }
                };

                if let Err(e) = decoder.write_all(&downloaded_bytes) {
                    tx.send(Err(e.into())).ok();
                    return;
                }

                // Drain whatever the decoder has produced so far without
                // waiting for the rest of the response.
                let produced = std::mem::take(decoder.get_mut());
                if !produced.is_empty() && tx.send(Ok(produced)).is_err() {
                    // Receiver dropped; no point continuing.
                    return;
                }
            }

            match decoder.finish() {
                Ok(remaining) => {
                    if !remaining.is_empty() {
                        tx.send(Ok(remaining)).ok();
                    }
                }
                Err(e) => {
                    tx.send(Err(e.into())).ok();
                }
            }
        });

        rx
    }
    pub async fn get_and_decode<T: DeserializeOwned>(
        &self,
        url: &str,
        auth_token: &str,
    ) -> Result<T, ClientError> {
        let response = self.send_get(url, Some(auth_token)).await?;
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
        let response = self.send_get(url, Some(auth_token)).await?;
        let response_text = response.text().await?;
        let result: T = serde_json::from_str(&response_text)?;
        Ok(result)
    }
}
