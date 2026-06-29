use serde::{Deserialize, Serialize};

use crate::downloader::{DownloadManager, error::DownloadError};

#[derive(Serialize, Deserialize, Debug)]
pub struct BuildMetadata {
    pub dependencies: Option<Vec<String>>,
    pub depots: Vec<Depot>,
    #[serde(alias = "clientId")]
    pub client_id: String,
    #[serde(alias = "clientSecret")]
    pub client_secret: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Depot {
    pub manifest: String,
    pub size: u64,
    #[serde(alias = "compressedSize")]
    pub compressed_size: u64,
    #[serde(alias = "productId")]
    pub product_id: String,
}

impl BuildMetadata {
    pub async fn get_build_metadata(
        download_manager: &DownloadManager,
        game_link: &str,
    ) -> Result<Self, DownloadError> {
        let auth = {
            let lock = download_manager.inner.lock().await;
            if let None = lock.auth.get_auth().await {
                return Err(DownloadError::NotAuthenticated);
            }
            lock.auth.get_auth().await.unwrap()
        };

        let game_details: BuildMetadata = match download_manager
            .client
            .get_and_decode::<BuildMetadata>(&game_link, &auth.access_token)
            .await
            .map_err(DownloadError::from)
        {
            Ok(game_details) => game_details,
            Err(DownloadError::Unauthorized) => {
                // Token refresh logic
                let auth = {
                    let lock = download_manager.inner.lock().await;
                    if let Err(_err) = lock.auth.refresh_auth().await {
                        return Err(DownloadError::Unauthorized);
                    }
                    lock.auth.get_auth().await.unwrap()
                };
                match download_manager
                    .client
                    .get_and_decode::<BuildMetadata>(&game_link, &auth.access_token)
                    .await
                    .map_err(DownloadError::from)
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
        Ok(game_details)
    }
}
