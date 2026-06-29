use serde::{Deserialize, Serialize};

use crate::downloader::{DownloadError, DownloadManager};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct DepotInfo {
    pub depot: DepotItems,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct DepotItems {
    pub items: Vec<DepotFile>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Chunk {
    pub md5: String,
    pub size: u64,
    #[serde(alias = "compressedMd5")]
    pub compressed_md5: String,
    #[serde(alias = "compressedSize")]
    pub compressed_size: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct DepotFile {
    pub md5: Option<String>,
    pub sha256: Option<String>,
    pub path: String,
    pub chunks: Option<Vec<Chunk>>,
    #[serde(alias = "type")]
    pub file_type: String,
}

impl DepotInfo {
    pub async fn get_depot_info(
        download_manager: &DownloadManager,
        depot_manifest: &str,
    ) -> Result<DepotInfo, DownloadError> {
        let auth = {
            let lock = download_manager.inner.lock().await;
            if let None = lock.auth.get_auth().await {
                return Err(DownloadError::NotAuthenticated);
            }
            lock.auth.get_auth().await.unwrap()
        };
        let url = format!(
            "https://cdn.gog.com/content-system/v2/meta/{}/{}/{}",
            &depot_manifest[0..2],
            &depot_manifest[2..4],
            &depot_manifest
        );

        let game_details: DepotInfo = match download_manager
            .client
            .get_and_decode::<DepotInfo>(&url, &auth.access_token)
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
                    .get_and_decode::<DepotInfo>(&url, &auth.access_token)
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
