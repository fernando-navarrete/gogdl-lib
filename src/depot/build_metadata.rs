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
    /// The language-filtered metadata for the build at `game_link`. The unfiltered document is
    /// cached by link, so a later language selection still works. The lock is held only for the
    /// cache read and insert, so concurrent misses may each fetch.
    pub async fn get_build_metadata(
        download_manager: &DepotManager,
        game_link: &str,
    ) -> Result<Self, DepotError> {
        let cached = download_manager
            .inner
            .lock()
            .await
            .build_metadata
            .get(game_link)
            .cloned();
        let mut build_metadata = match cached {
            Some(build_metadata) => build_metadata,
            None => {
                let fetched: BuildMetadata = download_manager
                    .client
                    .fetch(game_link, true, true, None)
                    .await?;
                download_manager
                    .inner
                    .lock()
                    .await
                    .build_metadata
                    .insert(game_link.to_string(), fetched.clone());
                fetched
            }
        };

        build_metadata.filter_languages(&["en-US", "en"]);
        Ok(build_metadata)
    }
    pub fn filter_languages(&mut self, languages: &[&str]) {
        let filtered_depots = self
            .depots
            .iter()
            .filter(|&depot| {
                languages
                    .iter()
                    .any(|&lang| depot.languages.iter().any(|l| l == lang))
                    || depot.languages.contains(&"*".to_string())
            })
            .cloned()
            .collect::<Vec<_>>();
        self.depots = filtered_depots;
    }
}
