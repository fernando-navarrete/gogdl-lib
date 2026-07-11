use std::io::{Read, Write};

use flate2::Compression;
use flate2::read::ZlibDecoder;
use flate2::write::GzEncoder;
use flate2::write::ZlibDecoder as ZlibStreamDecoder;
use futures::{StreamExt, stream};
use md5::{Digest, Md5};
use reqwest::Client;
use reqwest::header::{ACCEPT, CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_TYPE, ETAG, EXPECT, USER_AGENT};
use serde::de::DeserializeOwned;
use tokio::sync::mpsc;
use url::Url;

use crate::client::error::ClientError;

/// Header names GOG's cloud-storage service reads/writes for save-file
/// metadata; not in `reqwest::header`'s standard set, so named here once.
const X_OBJECT_META_LOCAL_LAST_MODIFIED: &str = "x-object-meta-locallastmodified";
const X_OBJECT_META_USER_AGENT: &str = "X-Object-Meta-User-Agent";
/// GOG Galaxy's own client identifies itself with this string on cloud-save
/// transfers; the storage backend appears to key some behavior off it, so it
/// is reproduced verbatim rather than gogdl-lib2's own identity.
const GALAXY_USER_AGENT: &str = "GOGGalaxyCommunicationService/2.0.4.164 (Windows_32bit)";

/// The raw result of downloading a save file: still gzip-compressed exactly
/// as GOG's cloud storage stores it, plus the two headers callers need to
/// verify integrity (`md5`, from the response `ETag`) and restore local
/// mtimes (`last_modified`, from a custom `X-Object-Meta-*` header).
/// Decompression and hash verification are the caller's responsibility, so
/// this type stays a thin transport result rather than a decoded save.
pub struct SaveDownload {
    pub bytes: Vec<u8>,
    pub md5: Option<String>,
    pub last_modified: Option<String>,
}

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
    /// Like `get_json`, but attaches caller-supplied headers instead of a
    /// bearer token. GitHub's API rejects unauthenticated requests that don't
    /// identify a `User-Agent`, which isn't a concern any other caller of
    /// this client has had to handle.
    pub async fn get_json_with_headers<T: DeserializeOwned>(
        &self,
        url: &str,
        headers: &[(&str, &str)],
    ) -> Result<T, ClientError> {
        let url = reqwest::Url::parse(url)?;
        let mut request = self.client.get(url);
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let response = request.send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await?;
            return Err(ClientError::Http { status, body });
        }
        let response_text = response.text().await?;
        let result: T = serde_json::from_str(&response_text)?;
        Ok(result)
    }
    /// Downloads a plain (uncompressed-at-the-HTTP-layer) file into memory,
    /// reporting cumulative progress as `on_progress(downloaded, total)`
    /// after each network-level chunk. `total` is the `Content-Length`
    /// header, or 0 if absent. Unlike `download_save`, this doesn't assume
    /// gzip content or any GOG-specific headers, so it suits any plain file
    /// download (e.g. a GitHub release asset).
    pub async fn download_bytes(
        &self,
        url: &str,
        headers: &[(&str, &str)],
        on_progress: impl Fn(u64, u64),
    ) -> Result<Vec<u8>, ClientError> {
        let parsed_url = reqwest::Url::parse(url)?;
        let mut request = self.client.get(parsed_url);
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let response = request.send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await?;
            return Err(ClientError::Http { status, body });
        }

        let total = response
            .headers()
            .get(CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0);

        let mut stream = response.bytes_stream();
        let mut buffer = Vec::new();
        let mut downloaded: u64 = 0;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            downloaded += chunk.len() as u64;
            on_progress(downloaded, total);
            buffer.extend_from_slice(&chunk);
        }

        Ok(buffer)
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
    /// Authenticated GET returning the raw response body as text, unlike
    /// `get_json_with_auth` this doesn't parse it: cloud storage's save-file
    /// listing is a newline-delimited plain-text body, not JSON.
    pub async fn get_text_with_auth(&self, url: &str, auth_token: &str) -> Result<String, ClientError> {
        let response = self.send_get(url, Some(auth_token)).await?;
        let response_text = response.text().await?;
        Ok(response_text)
    }
    /// Downloads a save file's raw (still gzip-compressed) bytes, reporting
    /// cumulative progress as `on_progress(downloaded, total)` after each
    /// network-level chunk. `total` is the `Content-Length` header, or 0 if
    /// absent. Decompression and MD5 verification are left to the caller
    /// (see `SaveDownload`).
    pub async fn download_save(
        &self,
        url: &str,
        auth_token: &str,
        on_progress: impl Fn(u64, u64),
    ) -> Result<SaveDownload, ClientError> {
        let response = self.send_get(url, Some(auth_token)).await?;
        let headers = response.headers().clone();
        let total = headers
            .get(CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0);
        let last_modified = headers
            .get(X_OBJECT_META_LOCAL_LAST_MODIFIED)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_owned());
        let md5 = headers
            .get(ETAG)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_owned());

        let mut stream = response.bytes_stream();
        let mut buffer = Vec::new();
        let mut downloaded: u64 = 0;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            downloaded += chunk.len() as u64;
            on_progress(downloaded, total);
            buffer.extend_from_slice(&chunk);
        }

        Ok(SaveDownload {
            bytes: buffer,
            md5,
            last_modified,
        })
    }
    /// Uploads a save file: gzip-compresses `uncompressed_body`, sets its MD5
    /// as the request's `ETag`, and PUTs it to `url` with the header set
    /// GOG's cloud storage expects (including a spoofed Galaxy client
    /// `User-Agent` — see `GALAXY_USER_AGENT`). Reports cumulative bytes sent
    /// as `on_progress(sent, total)` while streaming the compressed body in
    /// 16 KiB chunks.
    pub async fn upload_save(
        &self,
        url: &str,
        auth_token: &str,
        timestamp: &str,
        uncompressed_body: Vec<u8>,
        on_progress: impl Fn(u64, u64) + Send + Sync + 'static,
    ) -> Result<(), ClientError> {
        let mut encoder = GzEncoder::new(Vec::new(), Compression::new(6));
        encoder.write_all(&uncompressed_body)?;
        let mut gzipped = encoder.finish()?;
        let total = gzipped.len() as u64;

        let mut hasher = Md5::new();
        hasher.update(&gzipped);
        let etag = hex::encode(hasher.finalize());

        let url = reqwest::Url::parse(url)?;
        let request = self
            .client
            .put(url)
            .header(X_OBJECT_META_LOCAL_LAST_MODIFIED, timestamp)
            .header(ETAG, etag)
            .header(CONTENT_ENCODING, "gzip")
            .header(CONTENT_LENGTH, total)
            .header(EXPECT, "100-continue")
            .header(ACCEPT, "*/*")
            .header(X_OBJECT_META_USER_AGENT, GALAXY_USER_AGENT)
            .header(USER_AGENT, GALAXY_USER_AGENT)
            .header(CONTENT_TYPE, "application/octet-stream")
            .bearer_auth(auth_token);

        let mut sent: u64 = 0;
        let body_stream = stream::iter(std::iter::from_fn(move || {
            if gzipped.is_empty() {
                return None;
            }
            let chunk_len = gzipped.len().min(16 * 1024);
            let chunk = gzipped.drain(..chunk_len).collect::<Vec<_>>();
            sent += chunk.len() as u64;
            on_progress(sent, total);
            Some(Ok::<_, std::io::Error>(chunk))
        }));
        let body = reqwest::Body::wrap_stream(body_stream);

        let response = request.body(body).send().await?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await?;
            return Err(ClientError::Http { status, body });
        }
        Ok(())
    }
    /// Authenticated DELETE. Used for removing a save file from cloud
    /// storage; see the caveat on `saves::SaveFile::delete` — this HTTP verb
    /// is unverified against a live GOG account.
    pub async fn delete_with_auth(&self, url: &str, auth_token: &str) -> Result<(), ClientError> {
        let url = reqwest::Url::parse(url)?;
        let response = self.client.delete(url).bearer_auth(auth_token).send().await?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await?;
            return Err(ClientError::Http { status, body });
        }
        Ok(())
    }
}
