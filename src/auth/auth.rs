use std::sync::Arc;

use tokio::sync::Mutex;

use crate::{
    auth::{error::AuthError, model::Auth},
    client::HttpClient,
    constants::{AUTH_URL, LOGIN_URL, REFRESH_URL},
};

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
    pub async fn login_with_code(&self, code: &str) -> Result<(), AuthError> {
        let url = format!("{AUTH_URL}&code={code}");
        let mut response = match self.inner.lock().await.client.get_json::<Auth>(&url).await {
            Ok(auth) => auth,
            Err(err) => return Err(AuthError::from(err)),
        };
        response.valid_until = Some(response.expires_in as i64 + chrono::Utc::now().timestamp());
        self.inner.lock().await.tokens = Some(response);
        Ok(())
    }
    pub async fn refresh_auth(&self) -> Result<(), AuthError> {
        let tokens = {
            let tokens = self.inner.lock().await.tokens.clone();
            if tokens.is_none() {
                return Err(AuthError::Unauthorized);
            }
            tokens.unwrap()
        };
        let refresh_token = tokens.refresh_token;
        let url = format!("{REFRESH_URL}?refresh_token={refresh_token}");
        let mut response = match self.inner.lock().await.client.get_json::<Auth>(&url).await {
            Ok(auth) => auth,
            Err(err) => return Err(AuthError::from(err)),
        };
        response.valid_until = Some(response.expires_in as i64 + chrono::Utc::now().timestamp());
        self.inner.lock().await.tokens = Some(response);
        Ok(())
    }
    pub async fn get_auth(&self) -> Option<Auth> {
        self.inner.lock().await.tokens.clone()
    }
    pub async fn set_auth(&self, auth: Auth) {
        self.inner.lock().await.tokens = Some(auth);
    }
}
