use serde::{Deserialize, Serialize};

use crate::depot::{depot_manager::DepotManager, error::DepotError};

#[derive(Serialize, Deserialize, Debug, Clone)]
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
        {
            let lock = download_manager.inner.lock().await;
            if let Some(build_metadata) = lock.build_metadata.get(game_link) {
                return Ok(build_metadata.clone());
            }
        }

        let auth = {
            let lock = download_manager.inner.lock().await;
            lock.auth.clone()
        };

        let mut game_details: BuildMetadata = auth
            .authorized_get_and_decode(&download_manager.client, game_link)
            .await?;
        game_details.filter_languages("en-US");
        for depot in &mut game_details.depots {
            println!("{:?}", depot);
        }

        let mut lock = download_manager.inner.lock().await;
        lock.build_metadata
            .insert(game_link.to_string(), game_details.clone());
        Ok(game_details)
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
