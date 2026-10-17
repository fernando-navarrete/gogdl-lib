use std::{io::Read, sync::Arc};

use bytes::Bytes;
use flate2::read::ZlibDecoder;
use futures_util::{StreamExt, future::BoxFuture};
use reqwest::header::HeaderMap;
use reqwest::{Client, RequestBuilder, Response};
use serde::de::DeserializeOwned;

use crate::{
    client::retry::{MAX_ATTEMPTS, retry_or_return},
    client::{
        TokenObserver,
        auth::{AuthError, AuthManager, TokenFailure},
        error::ClientError,
    },
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
    #[cfg(test)]
    pub fn with_auth_manager(client: Client, auth_manager: AuthManager) -> Self {
        Self {
            client,
            auth_manager,
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
                    if status == reqwest::StatusCode::UNAUTHORIZED && require_auth {
                        self.auth_manager.refresh_auth(self).await?;
                    } else {
                        return Err(ClientError::HttpError { status, body });
                    }
                }
                Err(ClientError::AuthError(AuthError::TokenExpired)) => {
                    self.auth_manager.refresh_auth(self).await?;
                }
                Err(ClientError::NetworkError(err)) => {
                    retry_or_return(attempt, MAX_ATTEMPTS, ClientError::NetworkError(err)).await?;
                }
                Err(err) => {
                    return Err(err);
                }
            }
        }
        Err(ClientError::MaxRetriesReached)
    }
    /// POSTs `form` to a token endpoint as `application/x-www-form-urlencoded`
    /// and decodes the JSON reply. Not retried, no bearer token.
    ///
    /// Credentials belong in `form`, not in `url`. Every `reqwest::Error` is
    /// stripped of its URL before it is returned, so no error from here can
    /// print a secret, even one a caller put in the query string.
    pub(crate) async fn post_token<T: DeserializeOwned>(
        &self,
        url: &str,
        form: &[(&str, &str)],
    ) -> Result<T, TokenFailure> {
        let network = |e: reqwest::Error| TokenFailure::Network(e.without_url());
        let response = send_checked(self.client.post(url).form(form))
            .await
            .map_err(|failure| match failure {
                SendFailure::Network(e) => network(e),
                SendFailure::Status { status, body } => TokenFailure::Status { status, body },
            })?;
        let text = response.text().await.map_err(network)?;
        serde_json::from_str(&text).map_err(TokenFailure::Decode)
    }
    /// GETs `url` (with `headers`, if any), passes each body chunk to `f` as
    /// it arrives, and hands back the response headers once the body has been
    /// fully streamed. Not retried.
    ///
    /// # Errors
    /// [`ClientError::ChunkStreamCallbackError`] if `f` fails.
    pub async fn stream_url<'a>(
        &'a self,
        url: &str,
        headers: Option<&[(&str, &str)]>,
        mut f: impl FnMut(Bytes) -> BoxFuture<'a, std::io::Result<()>>,
    ) -> Result<HeaderMap, ClientError> {
        let url = reqwest::Url::parse(url)?;
        let mut response = send_checked(apply_headers(self.client.get(url), headers)).await?;

        let response_headers = std::mem::take(response.headers_mut());
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
        let request = apply_headers(
            self.client.put(url).header("Content-Length", body.len()),
            headers,
        );

        let mut remaining = Bytes::from(body);
        let chunks = futures_util::stream::iter(std::iter::from_fn(move || {
            if remaining.is_empty() {
                return None;
            }
            let chunk = remaining.split_to(remaining.len().min(CHUNK_SIZE));
            on_progress(chunk.len());
            Some(Ok::<_, std::io::Error>(chunk))
        }));

        send_checked(request.body(reqwest::Body::wrap_stream(chunks))).await?;
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
            Ok(result)
        } else {
            let result = self.get_json(url, headers).await?;
            Ok(result)
        }
    }
    async fn get_json<T: DeserializeOwned>(
        &self,
        url: &str,
        headers: Option<&[(&str, &str)]>,
    ) -> Result<T, ClientError> {
        let url = reqwest::Url::parse(url)?;
        let response = send_checked(apply_headers(self.client.get(url), headers)).await?;
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
        let request = apply_headers(self.client.get(url).bearer_auth(auth_token), headers);
        let response = send_checked(request).await?;

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
        let request = apply_headers(self.client.get(url).bearer_auth(auth_token), headers);
        let response = send_checked(request).await?;
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

/// Why a request from [`send_checked`] failed.
enum SendFailure {
    Network(reqwest::Error),
    Status {
        status: reqwest::StatusCode,
        body: String,
    },
}

impl From<SendFailure> for ClientError {
    fn from(failure: SendFailure) -> Self {
        match failure {
            SendFailure::Network(e) => ClientError::NetworkError(e),
            SendFailure::Status { status, body } => ClientError::HttpError { status, body },
        }
    }
}

/// Sends `request`; a non-success status becomes [`SendFailure::Status`] with
/// the body read as text.
async fn send_checked(request: RequestBuilder) -> Result<Response, SendFailure> {
    let response = request.send().await.map_err(SendFailure::Network)?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.map_err(SendFailure::Network)?;
        return Err(SendFailure::Status { status, body });
    }
    Ok(response)
}

fn apply_headers(mut request: RequestBuilder, headers: Option<&[(&str, &str)]>) -> RequestBuilder {
    if let Some(headers) = headers {
        for (key, value) in headers {
            request = request.header(*key, *value);
        }
    }
    request
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{ChunkServer, Reply};

    fn json_client() -> HttpClient {
        HttpClient::new_with_client(reqwest::Client::new())
    }

    #[tokio::test]
    async fn unauthenticated_401_returns_at_once() {
        let server = ChunkServer::start().await;
        server.script(
            "/rel",
            vec![Reply::StatusBody(401, b"Bad credentials".to_vec())],
        );
        let url = format!("{}/rel", server.base_url());
        let err = json_client()
            .fetch::<serde_json::Value>(&url, false, false, None)
            .await
            .err()
            .unwrap();
        match err {
            ClientError::HttpError { status, body } => {
                assert_eq!(status, reqwest::StatusCode::UNAUTHORIZED);
                assert_eq!(body, "Bad credentials");
            }
            other => panic!("expected HttpError, got {other:?}"),
        }
        assert_eq!(server.requests("/rel"), 1);
    }

    #[tokio::test]
    async fn http_5xx_is_not_retried() {
        let server = ChunkServer::start().await;
        server.script("/rel", vec![Reply::Status(503)]);
        let url = format!("{}/rel", server.base_url());
        let err = json_client()
            .fetch::<serde_json::Value>(&url, false, false, None)
            .await
            .err()
            .unwrap();
        assert!(matches!(
            err,
            ClientError::HttpError { status, .. } if status == reqwest::StatusCode::SERVICE_UNAVAILABLE
        ));
        assert_eq!(server.requests("/rel"), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn transport_errors_are_retried() {
        let server = ChunkServer::start().await;
        server.script(
            "/rel",
            vec![Reply::Close, Reply::Close, Reply::Body(b"{}".to_vec())],
        );
        let url = format!("{}/rel", server.base_url());
        json_client()
            .fetch::<serde_json::Value>(&url, false, false, None)
            .await
            .unwrap();
        assert_eq!(server.requests("/rel"), 3);
    }

    // Pins `.without_url()` on its own: the secret is in the URL here.
    #[tokio::test]
    async fn post_token_transport_errors_carry_no_url() {
        let server = ChunkServer::start().await;
        server.script("/token?refresh_token=URL-SENTINEL", vec![Reply::Close]);
        let url = format!("{}/token?refresh_token=URL-SENTINEL", server.base_url());
        let client = HttpClient::new_with_client(reqwest::Client::new());
        let err = client
            .post_token::<serde_json::Value>(&url, &[("grant_type", "refresh_token")])
            .await
            .err()
            .unwrap();
        assert!(matches!(err, TokenFailure::Network(_)));
        let TokenFailure::Network(e) = err else {
            unreachable!()
        };
        for text in [e.to_string(), format!("{e:?}")] {
            assert!(!text.contains("URL-SENTINEL"), "leaked: {text}");
        }
    }

    // A dead refresh token reaches the caller of an authenticated fetch as
    // `SessionRevoked`, whether the access token expired locally or the
    // resource answered 401.
    #[tokio::test]
    async fn a_dead_session_surfaces_from_an_authenticated_fetch() {
        let fixture = include_str!("../../tests/fixtures/token_error_refresh_garbage.json");
        for (valid_until, resource_requests) in [(0, 0), (i64::MAX, 1)] {
            let server = ChunkServer::start().await;
            server.script(
                "/token",
                vec![Reply::StatusBody(400, fixture.as_bytes().to_vec())],
            );
            server.script("/rel", vec![Reply::Status(401)]);
            let client = HttpClient::with_auth_manager(
                reqwest::Client::new(),
                AuthManager::with_token_url(format!("{}/token", server.base_url())),
            );
            let auth = crate::Auth::from_string(&format!(
                r#"{{"access_token":"a","refresh_token":"r","expires_in":3600,"token_type":"bearer","session_id":"s","user_id":"1","valid_until":{valid_until}}}"#
            ))
            .unwrap();
            client
                .restore_auth_from_string(&auth.to_string().unwrap())
                .await
                .unwrap();
            let err = client
                .fetch::<serde_json::Value>(
                    &format!("{}/rel", server.base_url()),
                    false,
                    true,
                    None,
                )
                .await
                .err()
                .unwrap();
            assert!(
                matches!(
                    err,
                    ClientError::AuthError(AuthError::SessionRevoked { .. })
                ),
                "{err:?}"
            );
            assert_eq!(server.requests("/token"), 1);
            assert_eq!(server.requests("/rel"), resource_requests);
        }
    }
}
