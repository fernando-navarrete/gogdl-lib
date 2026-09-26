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

/// One chunk of a [`DepotFile`]: a contiguous slice of the file that GOG's CDN serves as a single
/// zlib-compressed blob, addressed by the MD5 of its compressed bytes.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Chunk {
    /// MD5 of the chunk's uncompressed bytes, as a hex string.
    pub md5: String,
    /// Size of the chunk once decompressed, in bytes.
    pub size: u64,
    /// MD5 of the chunk's compressed bytes as a hex string; this is what the CDN URL is built from.
    #[serde(alias = "compressedMd5")]
    pub compressed_md5: String,
    /// Size of the chunk as transferred (compressed), in bytes.
    #[serde(alias = "compressedSize")]
    pub compressed_size: u64,
}

/// One entry of a product's depot manifest, as listed in
/// [`ProductBundle::product_files`](crate::ProductBundle::product_files).
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct DepotFile {
    /// MD5 of the whole file, when the manifest carries one.
    pub md5: Option<String>,
    /// Path relative to the install directory, as the manifest spells it (Windows separators
    /// included; the downloader normalizes and sanitizes it).
    pub path: String,
    /// The file's chunks in order, or `None` for an entry with no content (such as a directory).
    pub chunks: Option<Vec<Chunk>>,
    /// The manifest's `type` field. `"DepotFile"` marks a regular game file; any other value
    /// (redistributables, for one) is served from a different CDN path.
    #[serde(alias = "type")]
    pub file_type: String,
}

impl DepotFile {
    /// The file's uncompressed size, summed over its chunks, or `None` if it has no chunks.
    pub fn size(&self) -> Option<u64> {
        self.chunks
            .as_ref()
            .map(|chunks| chunks.iter().map(|chunk| chunk.size).sum())
    }
    pub(crate) fn to_download_units(&self, product_id: String) -> Vec<DownloadUnit> {
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
