use std::io::{Read, Write};
use std::time::Duration;

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
    ///
    /// `response_timeout` bounds how long the initial request may take to
    /// receive a response (guards against a CDN host that never answers);
    /// `idle_timeout` bounds each individual read of the body stream (guards
    /// against a connection that answered but then stalled mid-transfer,
    /// which a response-only timeout would never catch). Without both of
    /// these a single wedged connection would occupy its concurrency slot
    /// forever.
    pub fn fetch_chunk_stream(
        &self,
        url: &str,
        auth: String,
        response_timeout: Duration,
        idle_timeout: Duration,
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

            // `response_timeout` bounds only the wait for the initial
            // response (status + headers); the body is read afterward in
            // the loop below, governed solely by `idle_timeout` per read.
            // Deliberately not using reqwest's `RequestBuilder::timeout()`
            // here: despite its name, that's a *total* request timeout that
            // keeps counting down through the whole body read, so it would
            // abort a large-but-still-progressing chunk transfer once its
            // cumulative duration crosses `response_timeout`, even with no
            // actual stall and `idle_timeout` never coming into play.
            let response = match tokio::time::timeout(response_timeout, request.send()).await {
                Ok(Ok(response)) => response,
                Ok(Err(e)) => {
                    let err = if e.is_timeout() {
                        ClientError::Timeout
                    } else {
                        e.into()
                    };
                    tx.send(Err(err)).ok();
                    return;
                }
                Err(_elapsed) => {
                    tx.send(Err(ClientError::Timeout)).ok();
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
            loop {
                let next = match tokio::time::timeout(idle_timeout, stream.next()).await {
                    Ok(next) => next,
                    Err(_elapsed) => {
                        tx.send(Err(ClientError::Timeout)).ok();
                        return;
                    }
                };
                let chunk = match next {
                    Some(chunk) => chunk,
                    None => break,
                };
                let downloaded_bytes = match chunk {
                    Ok(bytes) => bytes,
                    Err(err) => {
                        tx.send(Err(ClientError::StreamError(err))).ok();
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

#[cfg(test)]
mod tests {
    //! Verifies the timeout behavior added to `fetch_chunk_stream`: a
    //! stalled CDN connection used to occupy its download slot forever
    //! (nothing in the old code ever gave up on it). These tests run a
    //! bare-bones local TCP server that either never answers, or answers
    //! and then goes silent mid-transfer, and check that both cases now
    //! surface `ClientError::Timeout` instead of hanging.
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    async fn bind_local() -> (TcpListener, String) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        (listener, format!("http://{addr}/chunk"))
    }

    #[tokio::test]
    async fn fetch_chunk_stream_times_out_when_server_never_responds() {
        let (listener, url) = bind_local().await;
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            // Read (and discard) the request, then just hang without ever
            // writing a response — the CDN host that never answers.
            let mut buf = [0u8; 1024];
            let _ = socket.read(&mut buf).await;
            tokio::time::sleep(Duration::from_secs(5)).await;
        });

        let client = HttpClient::new_with_client(reqwest::Client::new());
        let mut rx = client.fetch_chunk_stream(
            &url,
            String::new(),
            Duration::from_millis(150),
            Duration::from_secs(5),
        );

        match rx.recv().await {
            Some(Err(ClientError::Timeout)) => {}
            other => panic!("expected a response timeout, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn fetch_chunk_stream_times_out_when_stream_stalls_mid_transfer() {
        let (listener, url) = bind_local().await;

        let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(b"hello from the stalled cdn").unwrap();
        let compressed = encoder.finish().unwrap();

        tokio::spawn({
            let compressed = compressed.clone();
            async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut buf = [0u8; 1024];
                let _ = socket.read(&mut buf).await;

                // Promise more bytes than we'll ever send, so the body is
                // still "open" from the client's point of view.
                let header = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    compressed.len() + 1000
                );
                socket.write_all(header.as_bytes()).await.unwrap();
                socket.write_all(&compressed).await.unwrap();
                socket.flush().await.unwrap();

                // Stall: never send the rest, never close the connection —
                // the CDN connection that answers, then wedges mid-transfer.
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        });

        let client = HttpClient::new_with_client(reqwest::Client::new());
        let mut rx = client.fetch_chunk_stream(
            &url,
            String::new(),
            Duration::from_secs(5),
            Duration::from_millis(750),
        );

        // No more bytes ever arrive after the (fully sent, but small)
        // compressed payload, so the idle timeout must eventually fire
        // instead of hanging forever — this is exactly the failure mode
        // ("one stalled connection occupies its concurrency slot forever
        // with no timeout to reclaim it") this change fixes. Whether the
        // small payload is decoded and surfaced before then is an
        // implementation detail of the streaming zlib decoder's internal
        // buffering, not something this test should pin down — only the
        // eventual timeout matters here.
        let mut saw_timeout = false;
        while let Some(message) = rx.recv().await {
            match message {
                Ok(_bytes) => continue,
                Err(ClientError::Timeout) => {
                    saw_timeout = true;
                    break;
                }
                Err(other) => panic!("unexpected error before the idle timeout: {other}"),
            }
        }
        assert!(
            saw_timeout,
            "expected the idle timeout to eventually fire for a stalled connection"
        );
    }
}
