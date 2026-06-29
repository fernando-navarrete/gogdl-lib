use std::sync::Arc;

use tokio::sync::Mutex;

use crate::{
    auth::AuthManager,
    client::HttpClient,
    secure_links::{error::SecureLinksError, secure_links::SecureLinks},
};

pub struct SecureLinksManager {
    pub inner: Arc<Mutex<SecureLinksManagerInner>>,
    pub client: HttpClient,
}

pub struct SecureLinksManagerInner {
    pub auth: AuthManager,
}

impl SecureLinksManager {
    pub fn new(client: HttpClient, auth: AuthManager) -> Self {
        Self {
            inner: Arc::new(Mutex::new(SecureLinksManagerInner { auth })),
            client,
        }
    }
    pub async fn get_secure_links(&self, game_id: i32) -> Result<SecureLinks, SecureLinksError> {
        let secure_links = SecureLinks::get_secure_links(self, game_id).await?;
        Ok(secure_links)
    }
}
