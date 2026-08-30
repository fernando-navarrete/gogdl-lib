use serde::{Deserialize, Serialize};
use url::Url;

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
        game_id: &str,
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
    pub fn get_highest_priority_url(&self) -> Result<&UrlFormat, SecureLinksError> {
        self.urls
            .iter()
            .max_by_key(|url| url.priority)
            .ok_or(SecureLinksError::NoSecureLink)
    }
}

impl UrlFormat {
    pub fn parse_url_redist(&self, chunk_hash: &str) -> String {
        let url = format!(
            "https://gog-cdn-fastly.gog.com/content-system/v2/dependencies/store/{}/{}/{}",
            &chunk_hash[0..2],
            &chunk_hash[2..4],
            chunk_hash
        );
        url
    }
    pub fn parse_url(&self, chunk_hash: &str) -> String {
        let mut url = self.url_format.clone();
        url = url.replace("{path}", &self.parameters.path);
        url = url.replace("{token}", &self.parameters.token);
        url = url.replace("{base_url}", &self.parameters.base_url);

        if let Some(expires_at) = self.parameters.expires_at {
            url = url.replace("{expires_at}", &expires_at.to_string());
        }
        if let Some(dirs) = self.parameters.dirs {
            url = url.replace("{dirs}", &dirs.to_string());
        }
        if let Some(ttl) = self.parameters.ttl {
            url = url.replace("{ttl}", &ttl.to_string());
        }
        if let Some(source) = &self.parameters.source {
            url = url.replace("{source}", source);
        }
        if let Some(gog_token) = &self.parameters.gog_token {
            url = url.replace("{gog_token}", gog_token);
        }
        if let Some(l) = &self.parameters.l {
            url = url.replace("{l}", l);
        }
        let galaxy_path = format!("{}/{}/{}", &chunk_hash[0..2], &chunk_hash[2..4], chunk_hash);

        // Properly insert chunk path into URL path component (before query string)
        if let Ok(mut parsed_url) = Url::parse(&url) {
            let current_path = parsed_url.path().trim_end_matches('/');
            let new_path = format!("{}/{}", current_path, galaxy_path);
            parsed_url.set_path(&new_path);
            parsed_url.to_string()
        } else {
            // Fallback to simple concatenation if URL parsing fails
            format!("{}/{}", url.trim_end_matches('/'), galaxy_path)
        }
    }
}
