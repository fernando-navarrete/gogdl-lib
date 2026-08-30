use serde::{Deserialize, Serialize};

use crate::depot::{depot_manager::DepotManager, error::DepotError};

#[derive(Serialize, Deserialize, Debug)]
pub struct BuildMetadata {
    pub dependencies: Option<Vec<String>>,
    pub depots: Vec<Depot>,
    #[serde(alias = "clientId")]
    pub client_id: String,
    #[serde(alias = "clientSecret")]
    pub client_secret: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Depot {
    pub manifest: String,
    pub size: u64,
    #[serde(alias = "compressedSize")]
    pub compressed_size: u64,
    #[serde(alias = "productId")]
    pub product_id: String,
    pub languages: Vec<String>,
}

impl BuildMetadata {
    pub async fn get_build_metadata(
        download_manager: &DepotManager,
        game_link: &str,
    ) -> Result<Self, DepotError> {
        let auth_manager = {
            let lock = download_manager.inner.lock().await;
            if let Err(err) = lock.auth.get_auth().await {
                return Err(DepotError::AuthError(err));
            }
            lock.auth.clone()
        };

        let mut build_metadata: BuildMetadata = download_manager
            .client
            .fetch(game_link, Some(auth_manager), true)
            .await?;

        build_metadata.filter_languages("en-US");
        Ok(build_metadata)
    }
    pub fn filter_languages(&mut self, language: &str) {
        let filtered_depots = self
            .depots
            .iter()
            .filter(|&depot| {
                depot.languages.contains(&language.to_string())
                    || depot.languages.contains(&"*".to_string())
            })
            .map(|depot| depot.clone())
            .collect::<Vec<_>>();
        self.depots = filtered_depots;
    }
}
