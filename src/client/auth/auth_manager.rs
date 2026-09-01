use std::sync::Arc;

use tokio::sync::Mutex;

use crate::{
    client::{
        ClientError, HttpClient,
        auth::{Auth, AuthError, TokenObserver},
    },
    constants::{AUTH_URL, LOGIN_URL, REFRESH_URL},
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
        let url = format!("{AUTH_URL}&code={code}");

        let mut auth: Auth = match client.fetch_no_retry(&url, false, false).await {
            Ok(auth) => auth,
            Err(ClientError::AuthError(e)) => return Err(e),
            Err(e) => {
                return Err(AuthError::ClientError {
                    inner: e.to_string(),
                });
            }
        };

        auth.valid_until = Some(auth.expires_in as i64 + chrono::Utc::now().timestamp());
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
        // Lock to prevent concurrent refresh attempts
        let _lock = self.refresh_lock.lock().await;
        let tokens = {
            let tokens = self.inner.lock().await.tokens.clone();
            if tokens.is_none() {
                return Err(AuthError::NotAuthenticated);
            }
            tokens.unwrap()
        };
        let refresh_token = tokens.refresh_token;
        let url = format!("{REFRESH_URL}&refresh_token={refresh_token}");

        let mut auth: Auth = match client.fetch_no_retry(&url, false, false).await {
            Ok(auth) => auth,
            Err(ClientError::AuthError(e)) => return Err(e),
            Err(e) => {
                return Err(AuthError::ClientError {
                    inner: e.to_string(),
                });
            }
        };
        auth.valid_until = Some(auth.expires_in as i64 + chrono::Utc::now().timestamp());

        {
            let mut inner = self.inner.lock().await;
            inner.tokens = Some(auth.clone());
            if let Some(observer) = &inner.token_observer {
                observer.on_token_refreshed(auth.clone());
            }
        }
        Ok(())
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
