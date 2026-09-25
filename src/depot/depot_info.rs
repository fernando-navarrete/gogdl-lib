use serde::{Deserialize, Serialize};

use crate::{
    depot::{depot_manager::DepotManager, error::DepotError},
    downloader::{DownloadUnit, FileType},
};

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
    pub path: String,
    pub chunks: Option<Vec<Chunk>>,
    #[serde(alias = "type")]
    pub file_type: String,
}

impl DepotFile {
    pub fn size(&self) -> Option<u64> {
        self.chunks
            .as_ref()
            .map(|chunks| chunks.iter().map(|chunk| chunk.size).sum())
    }
    pub fn to_download_units(&self, product_id: String) -> Vec<DownloadUnit> {
        let mut offset = 0;
        let mut units: Vec<DownloadUnit> = Vec::new();

        if let Some(chunks) = &self.chunks {
            for chunk in chunks.iter() {
                units.push(DownloadUnit {
                    product_id: product_id.clone(),
                    md5: chunk.md5.clone(),
                    size: chunk.size,
                    compressed_md5: chunk.compressed_md5.clone(),
                    _compressed_size: chunk.compressed_size,
                    path: self.path.clone(),
                    offset,
                    file_type: match self.file_type.as_ref() {
                        "DepotFile" => FileType::DepotFile,
                        _ => FileType::Other,
                    },
                });
                offset += chunk.size
            }
        }
        units
    }
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
            depot_manifest
        );

        let depot_info: DepotInfo = download_manager
            .client
            .fetch(&url, true, true, None)
            .await?;

        Ok(depot_info)
    }
}
