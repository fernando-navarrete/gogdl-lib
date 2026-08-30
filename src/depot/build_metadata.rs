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
        let auth = {
            let lock = download_manager.inner.lock().await;
            if let None = lock.auth.get_auth().await {
                return Err(DepotError::Unauthorized);
            }
            lock.auth.get_auth().await.unwrap()
        };

        let mut game_details: BuildMetadata = match download_manager
            .client
            .get_and_decode::<BuildMetadata>(&game_link, &auth.access_token)
            .await
            .map_err(DepotError::from)
        {
            Ok(game_details) => game_details,
            Err(DepotError::Unauthorized) => {
                // Token refresh logic
                let auth = {
                    let lock = download_manager.inner.lock().await;
                    if let Err(_err) = lock.auth.refresh_auth().await {
                        return Err(DepotError::Unauthorized);
                    }
                    lock.auth.get_auth().await.unwrap()
                };
                match download_manager
                    .client
                    .get_and_decode::<BuildMetadata>(&game_link, &auth.access_token)
                    .await
                    .map_err(DepotError::from)
                {
                    Ok(game_details) => game_details,
                    Err(err) => {
                        return Err(err);
                    }
                }
            }
            Err(err) => {
                return Err(err);
            }
        };
        game_details.filter_languages("en-US");
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
