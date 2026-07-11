use std::future::Future;
use std::sync::Arc;

use reqwest::StatusCode;
use serde::de::DeserializeOwned;
use tokio::sync::Mutex;

use crate::{
    auth::{error::AuthError, model::Auth},
    client::{ClientError, HttpClient},
    constants::{AUTH_URL, LOGIN_URL, REFRESH_URL},
};

/// The outcome of an authenticated fetch made through
/// `AuthManager::authorized_get_json` / `authorized_get_and_decode`.
/// Kept distinct from `ClientError` because "no token was ever available to
/// try" (never logged in / restored) is a precondition callers usually want
/// to surface as their own `NotAuthenticated`, not as a transport error.
pub enum AuthorizedFetchError {
    NotAuthenticated,
    Client(ClientError),
}

impl From<ClientError> for AuthorizedFetchError {
    fn from(err: ClientError) -> Self {
        AuthorizedFetchError::Client(err)
    }
}

#[derive(Clone)]
pub struct AuthManager {
    pub inner: Arc<Mutex<AuthManagerInner>>,
}

pub struct AuthManagerInner {
    pub client: HttpClient,
    pub tokens: Option<Auth>,
}

impl AuthManager {
    pub fn new(client: HttpClient) -> Self {
        Self {
            inner: Arc::new(Mutex::new(AuthManagerInner {
                client,
                tokens: None,
            })),
        }
    }
    pub fn get_login_url(&self) -> &str {
        LOGIN_URL
    }
    pub async fn login_with_code(&self, code: &str) -> Result<String, AuthError> {
        let url = format!("{AUTH_URL}&code={code}");
        let mut response = match self.inner.lock().await.client.get_json::<Auth>(&url).await {
            Ok(auth) => auth,
            Err(err) => return Err(AuthError::from(err)),
        };
        response.valid_until = Some(response.expires_in as i64 + chrono::Utc::now().timestamp());
        let json_str = response.to_string()?;
        self.inner.lock().await.tokens = Some(response);
        Ok(json_str)
    }
    pub async fn restore_from_string(&self, json_str: &str) -> Result<(), AuthError> {
        let tokens = Auth::from_string(json_str)?;
        self.inner.lock().await.tokens = Some(tokens);
        Ok(())
    }
    pub async fn refresh_auth(&self) -> Result<String, AuthError> {
        let tokens = {
            let tokens = self.inner.lock().await.tokens.clone();
            if tokens.is_none() {
                return Err(AuthError::Unauthorized);
            }
            tokens.unwrap()
        };
        let refresh_token = tokens.refresh_token;
        let url = format!("{REFRESH_URL}&refresh_token={refresh_token}");
        let mut response = match self.inner.lock().await.client.get_json::<Auth>(&url).await {
            Ok(auth) => auth,
            Err(err) => return Err(AuthError::from(err)),
        };
        let json_str = response.to_string()?;
        response.valid_until = Some(response.expires_in as i64 + chrono::Utc::now().timestamp());
        self.inner.lock().await.tokens = Some(response);
        Ok(json_str)
    }
    pub async fn get_auth(&self) -> Option<Auth> {
        self.inner.lock().await.tokens.clone()
    }
    pub async fn set_auth(&self, auth: Auth) {
        self.inner.lock().await.tokens = Some(auth);
    }
    /// Runs `try_once` with the current access token; if it comes back
    /// `Unauthorized` (401), refreshes the token once and retries `try_once`
    /// with the new one. Returns `NotAuthenticated` up front if there is no
    /// token to try at all yet.
    ///
    /// This centralizes the "fetch with token, refresh-and-retry on 401"
    /// dance that used to be hand-written in every domain type's `get_*`
    /// method.
    async fn authorized_fetch<T, Fut>(
        &self,
        mut try_once: impl FnMut(String) -> Fut,
    ) -> Result<T, AuthorizedFetchError>
    where
        Fut: Future<Output = Result<T, ClientError>>,
    {
        let token = self
            .get_auth()
            .await
            .ok_or(AuthorizedFetchError::NotAuthenticated)?
            .access_token;

        match try_once(token).await {
            Err(ClientError::Http { status, body }) if status == StatusCode::UNAUTHORIZED => {
                if self.refresh_auth().await.is_err() {
                    return Err(AuthorizedFetchError::Client(ClientError::Http {
                        status,
                        body,
                    }));
                }
                let token = self
                    .get_auth()
                    .await
                    .ok_or(AuthorizedFetchError::NotAuthenticated)?
                    .access_token;
                try_once(token).await.map_err(AuthorizedFetchError::from)
            }
            other => other.map_err(AuthorizedFetchError::from),
        }
    }
    /// Authenticated JSON GET via `client`, refreshing and retrying once on
    /// a 401. See `authorized_fetch`.
    pub async fn authorized_get_json<T: DeserializeOwned>(
        &self,
        client: &HttpClient,
        url: &str,
    ) -> Result<T, AuthorizedFetchError> {
        self.authorized_fetch(|token| {
            let client = client.clone();
            let url = url.to_string();
            async move { client.get_json_with_auth::<T>(&url, &token).await }
        })
        .await
    }
    /// Authenticated, zlib-decoded JSON GET via `client`, refreshing and
    /// retrying once on a 401. See `authorized_fetch`.
    pub async fn authorized_get_and_decode<T: DeserializeOwned>(
        &self,
        client: &HttpClient,
        url: &str,
    ) -> Result<T, AuthorizedFetchError> {
        self.authorized_fetch(|token| {
            let client = client.clone();
            let url = url.to_string();
            async move { client.get_and_decode::<T>(&url, &token).await }
        })
        .await
    }
}
