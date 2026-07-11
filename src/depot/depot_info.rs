use serde::{Deserialize, Serialize};

use crate::depot::{depot_manager::DepotManager, error::DepotError};

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
    pub chunks: Vec<Chunk>,
    #[serde(alias = "type")]
    pub file_type: String,
}

#[derive(Clone)]
pub struct DownloadUnit {
    pub md5: String,
    pub size: u64,
    pub compressed_md5: String,
    pub compressed_size: u64,
    pub path: String,
    pub offset: u64,
}

impl DepotFile {
    pub fn from_download_unit(unit: &DownloadUnit) -> Self {
        DepotFile {
            md5: Some(unit.md5.clone()),
            sha256: None,
            path: unit.path.clone(),
            chunks: Vec::new(),
            file_type: "".to_string(),
        }
    }

    pub fn get_download_units(&self) -> Vec<DownloadUnit> {
        let mut offset = 0;
        let mut units: Vec<DownloadUnit> = Vec::new();
        for chunk in self.chunks.iter() {
            units.push(DownloadUnit {
                md5: chunk.md5.clone(),
                size: chunk.size,
                compressed_md5: chunk.compressed_md5.clone(),
                compressed_size: chunk.compressed_size,
                path: self.path.clone(),
                offset,
            });
            offset += chunk.size;
        }
        units
    }
    pub fn get_file_size(&self) -> u64 {
        self.chunks.iter().map(|c| c.size).sum()
    }
}

impl DepotInfo {
    pub async fn get_depot_info(
        download_manager: &DepotManager,
        depot_manifest: &str,
    ) -> Result<DepotInfo, DepotError> {
        {
            let lock = download_manager.inner.lock().await;
            if let Some(depot_info) = lock.depot_info.get(depot_manifest) {
                return Ok(depot_info.clone());
            }
        }

        let auth = {
            let lock = download_manager.inner.lock().await;
            lock.auth.clone()
        };
        let url = format!(
            "https://cdn.gog.com/content-system/v2/meta/{}/{}/{}",
            &depot_manifest[0..2],
            &depot_manifest[2..4],
            &depot_manifest
        );

        let game_details: DepotInfo = auth
            .authorized_get_and_decode(&download_manager.client, &url)
            .await?;

        let mut lock = download_manager.inner.lock().await;
        lock.depot_info
            .insert(depot_manifest.to_string(), game_details.clone());
        Ok(game_details)
    }
}
