use std::sync::Arc;

use tokio::sync::Mutex;

use crate::{
    client::{
        ClientError, HttpClient,
        auth::{Auth, AuthError, TokenObserver},
    },
    constants::{CLIENT_ID, CLIENT_SECRET, LOGIN_URL, REDIRECT_URI, TOKEN_URL},
};

#[derive(Clone)]
pub struct AuthManager {
    inner: Arc<Mutex<AuthManagerInner>>,
    refresh_lock: Arc<Mutex<()>>,
}

pub struct AuthManagerInner {
    tokens: Option<Auth>,
    token_observer: Option<Arc<dyn TokenObserver>>,
}

impl AuthManager {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(AuthManagerInner {
                tokens: None,
                token_observer: None,
            })),
            refresh_lock: Arc::new(Mutex::new(())),
        }
    }
    pub fn get_login_url(&self) -> &str {
        LOGIN_URL
    }
    pub async fn set_token_observer(&self, observer: Arc<dyn TokenObserver>) {
        self.inner.lock().await.token_observer = Some(observer);
    }
    pub async fn remove_token_observer(&self) {
        self.inner.lock().await.token_observer = None;
    }
    pub async fn login_with_code(
        &self,
        code: &str,
        client: &HttpClient,
    ) -> Result<String, AuthError> {
        let auth = Self::exchange(
            client,
            TOKEN_URL,
            &[
                ("client_id", CLIENT_ID),
                ("client_secret", CLIENT_SECRET),
                ("grant_type", "authorization_code"),
                ("code", code),
                ("redirect_uri", REDIRECT_URI),
            ],
        )
        .await?;
        let json_str = auth.to_string()?;
        self.inner.lock().await.tokens = Some(auth);
        Ok(json_str)
    }
    pub async fn restore_from_string(&self, json_str: &str) -> Result<(), AuthError> {
        let tokens = Auth::from_string(json_str)?;
        self.inner.lock().await.tokens = Some(tokens);
        Ok(())
    }
    pub async fn refresh_auth(&self, client: &HttpClient) -> Result<(), AuthError> {
        // Snapshot before queueing on refresh_lock, so we can tell whether another
        // waiter refreshed while we waited.
        let stale_token = {
            let inner = self.inner.lock().await;
            match inner.tokens.as_ref() {
                Some(tokens) => tokens.access_token.clone(),
                None => return Err(AuthError::NotAuthenticated),
            }
        };

        // Lock to prevent concurrent refresh attempts
        let _lock = self.refresh_lock.lock().await;
        let tokens = {
            let tokens = self.inner.lock().await.tokens.clone();
            if tokens.is_none() {
                return Err(AuthError::NotAuthenticated);
            }
            tokens.unwrap()
        };

        // Another waiter already refreshed while we were queued — reuse its result.
        if tokens.access_token != stale_token {
            return Ok(());
        }

        let auth = Self::exchange(
            client,
            TOKEN_URL,
            &[
                ("client_id", CLIENT_ID),
                ("client_secret", CLIENT_SECRET),
                ("grant_type", "refresh_token"),
                ("refresh_token", &tokens.refresh_token),
            ],
        )
        .await?;

        {
            let mut inner = self.inner.lock().await;
            inner.tokens = Some(auth.clone());
            let observer = inner
                .token_observer
                .as_ref()
                .map(|observer| observer.clone());
            drop(inner);
            if let Some(observer) = observer {
                observer.on_token_refreshed(auth.clone());
            }
        }
        Ok(())
    }
    /// POSTs a token grant to `url` and stamps `valid_until`. The grant goes
    /// in a form body and the transport error has its URL stripped, so
    /// neither the code nor the refresh token can reach an error string.
    async fn exchange(
        client: &HttpClient,
        url: &str,
        form: &[(&str, &str)],
    ) -> Result<Auth, AuthError> {
        let mut auth: Auth = match client.post_token(url, form).await {
            Ok(auth) => auth,
            Err(ClientError::AuthError(e)) => return Err(e),
            Err(e) => {
                return Err(AuthError::ClientError {
                    inner: e.to_string(),
                });
            }
        };
        auth.valid_until = Some(auth.expires_in as i64 + chrono::Utc::now().timestamp());
        Ok(auth)
    }
    pub async fn get_auth(&self) -> Result<Auth, AuthError> {
        let auth = {
            let lock = self.inner.lock().await;
            if let Some(auth) = lock.tokens.as_ref() {
                auth.clone()
            } else {
                return Err(AuthError::NotAuthenticated);
            }
        };
        if auth.is_valid() {
            Ok(auth)
        } else {
            Err(AuthError::TokenExpired)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{ChunkServer, Reply};

    const REFRESH: &str = "REFRESH-SENTINEL";
    const CODE: &str = "CODE-SENTINEL";

    fn no_secrets(err: &AuthError) {
        for text in [err.to_string(), format!("{err:?}")] {
            assert!(text.contains("rror"), "not an error string: {text}");
            for secret in [REFRESH, CODE, CLIENT_SECRET] {
                assert!(!text.contains(secret), "{secret} leaked: {text}");
            }
        }
    }

    #[tokio::test]
    async fn a_failed_refresh_exchange_leaks_no_secret() {
        let server = ChunkServer::start().await;
        for reply in [Reply::Close, Reply::Status(400)] {
            server.script("/token", vec![reply]);
            let url = format!("{}/token", server.base_url());
            let err = AuthManager::exchange(
                &HttpClient::new_with_client(reqwest::Client::new()),
                &url,
                &[
                    ("client_secret", CLIENT_SECRET),
                    ("grant_type", "refresh_token"),
                    ("refresh_token", REFRESH),
                ],
            )
            .await
            .err()
            .unwrap();
            no_secrets(&err);
        }
    }

    #[tokio::test]
    async fn a_failed_login_exchange_leaks_no_secret() {
        let server = ChunkServer::start().await;
        server.script("/token", vec![Reply::Close]);
        let url = format!("{}/token", server.base_url());
        let err = AuthManager::exchange(
            &HttpClient::new_with_client(reqwest::Client::new()),
            &url,
            &[("client_secret", CLIENT_SECRET), ("code", CODE)],
        )
        .await
        .err()
        .unwrap();
        no_secrets(&err);
    }
}
