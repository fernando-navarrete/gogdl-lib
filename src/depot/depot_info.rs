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
    pub chunks: Option<Vec<Chunk>>,
    #[serde(alias = "type")]
    pub file_type: String,
}

impl DepotInfo {
    pub async fn get_depot_info(
        download_manager: &DepotManager,
        depot_manifest: &str,
    ) -> Result<DepotInfo, DepotError> {
        let url = format!(
            "https://cdn.gog.com/content-system/v2/meta/{}/{}/{}",
            &depot_manifest[0..2],
            &depot_manifest[2..4],
            &depot_manifest
        );

        let depot_info: DepotInfo = download_manager.client.fetch(&url, true, true).await?;

        Ok(depot_info)
    }
}
