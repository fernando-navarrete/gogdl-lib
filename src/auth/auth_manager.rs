use std::sync::Arc;

use tokio::sync::Mutex;

use crate::{
    auth::{auth::Auth, error::AuthError, token_observer::TokenObserver},
    client::{HttpClient, Request},
    constants::{AUTH_URL, LOGIN_URL, REFRESH_URL},
};

#[derive(Clone)]
pub struct AuthManager {
    inner: Arc<Mutex<AuthManagerInner>>,
    refresh_lock: Arc<Mutex<()>>,
}

pub struct AuthManagerInner {
    client: HttpClient,
    tokens: Option<Auth>,
    token_observer: Option<Arc<dyn TokenObserver>>,
}

impl AuthManager {
    pub fn new(client: HttpClient) -> Self {
        Self {
            inner: Arc::new(Mutex::new(AuthManagerInner {
                client,
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
    pub async fn login_with_code(&self, code: &str) -> Result<String, AuthError> {
        let url = format!("{AUTH_URL}&code={code}");

        let client = {
            let lock = self.inner.lock().await;
            lock.client.clone()
        };

        let mut auth: Auth = client.fetch(Request::Get { url }).await?;

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
    pub async fn refresh_auth(&self) -> Result<(), AuthError> {
        // Lock to prevent concurrent refresh attempts
        let _lock = self.refresh_lock.lock().await;
        let tokens = {
            let tokens = self.inner.lock().await.tokens.clone();
            if tokens.is_none() {
                return Err(AuthError::Unauthorized);
            }
            tokens.unwrap()
        };
        let refresh_token = tokens.refresh_token;
        let url = format!("{REFRESH_URL}&refresh_token={refresh_token}");

        let client = {
            let lock = self.inner.lock().await;
            lock.client.clone()
        };

        let auth: Auth = client.fetch(Request::Get { url }).await?;

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
                return Err(AuthError::Unauthorized);
            }
        };
        if auth.is_valid() {
            Ok(auth)
        } else {
            Err(AuthError::AuthExpired)
        }
    }
    pub async fn set_auth(&self, auth: Auth) {
        self.inner.lock().await.tokens = Some(auth);
    }
}
