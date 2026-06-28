use std::sync::Arc;

use tokio::sync::Mutex;

use crate::{
    auth::{error::AuthError, model::Auth},
    client::HttpClient,
    constants::{AUTH_URL, LOGIN_URL, REFRESH_URL},
};

pub struct AuthManager {
    pub client: HttpClient,
    pub tokens: Arc<Mutex<Option<Auth>>>,
}

impl AuthManager {
    pub fn new(client: HttpClient) -> Self {
        Self {
            client,
            tokens: Arc::new(Mutex::new(None)),
        }
    }
    pub fn get_login_url(&self) -> &str {
        LOGIN_URL
    }
    pub async fn login_with_code(&self, code: &str) -> Result<(), AuthError> {
        let url = format!("{AUTH_URL}?code={code}");
        let mut response = match self.client.get_json::<Auth>(&url).await {
            Ok(auth) => auth,
            Err(err) => return Err(AuthError::from(err)),
        };
        response.valid_until = Some(response.expires_in as i64 + chrono::Utc::now().timestamp());
        self.tokens.lock().await.replace(response);
        Ok(())
    }
    pub async fn refresh_auth(&self) -> Result<(), AuthError> {
        let tokens = {
            let tokens = self.tokens.lock().await.clone();
            if tokens.is_none() {
                return Err(AuthError::Unauthorized);
            }
            tokens.unwrap()
        };
        let refresh_token = tokens.refresh_token;
        let url = format!("{REFRESH_URL}?refresh_token={refresh_token}");
        let mut response = match self.client.get_json::<Auth>(&url).await {
            Ok(auth) => auth,
            Err(err) => return Err(AuthError::from(err)),
        };
        response.valid_until = Some(response.expires_in as i64 + chrono::Utc::now().timestamp());
        self.tokens.lock().await.replace(response);
        Ok(())
    }
    pub async fn get_auth(&self) -> Option<Auth> {
        self.tokens.lock().await.clone()
    }
    pub async fn set_auth(&self, auth: Auth) {
        self.tokens.lock().await.replace(auth);
    }
}
