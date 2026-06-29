use serde::{Deserialize, Serialize};

use crate::secure_links::{SecureLinksManager, error::SecureLinksError};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct CdnUrlParams {
    pub base_url: String,
    pub path: String,
    pub token: String,
    pub expires_at: Option<u64>,
    pub dirs: Option<u64>,
    pub ttl: Option<u64>,
    pub source: Option<String>,
    pub gog_token: Option<String>,
    pub l: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct UrlFormat {
    pub endpoint_name: String,
    pub url_format: String,
    pub priority: u64,
    pub parameters: CdnUrlParams,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SecureLinks {
    pub product_id: u64,
    pub urls: Vec<UrlFormat>,
}

impl SecureLinks {
    pub async fn get_secure_links(
        secure_links_manager: &SecureLinksManager,
        game_id: i32,
    ) -> Result<SecureLinks, SecureLinksError> {
        let auth = {
            let lock = secure_links_manager.inner.lock().await;
            if let None = lock.auth.get_auth().await {
                return Err(SecureLinksError::NotAuthenticated);
            }
            lock.auth.get_auth().await.unwrap()
        };
        let url = format!(
            "https://content-system.gog.com/products/{}/secure_link?generation=2&_version=2&path=/",
            game_id
        );
        let secure_links: SecureLinks = match secure_links_manager
            .client
            .get_json_with_auth::<SecureLinks>(&url, &auth.access_token)
            .await
            .map_err(SecureLinksError::from)
        {
            Ok(secure_links) => secure_links,
            Err(SecureLinksError::Unauthorized) => {
                // Token refresh logic
                let auth = {
                    let lock = secure_links_manager.inner.lock().await;
                    if let Err(_err) = lock.auth.refresh_auth().await {
                        return Err(SecureLinksError::Unauthorized);
                    }
                    lock.auth.get_auth().await.unwrap()
                };
                match secure_links_manager
                    .client
                    .get_json_with_auth::<SecureLinks>(&url, &auth.access_token)
                    .await
                    .map_err(SecureLinksError::from)
                {
                    Ok(secure_links) => secure_links,
                    Err(err) => {
                        return Err(err);
                    }
                }
            }
            Err(err) => {
                return Err(err);
            }
        };
        Ok(secure_links)
    }
}
