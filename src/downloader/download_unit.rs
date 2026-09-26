use crate::ProductBundle;

#[derive(Clone)]
pub enum FileType {
    DepotFile,
    Other,
}

#[derive(Clone)]
pub struct DownloadUnit {
    pub md5: String,
    pub size: u64,
    pub compressed_md5: String,
    pub compressed_size: u64,
    pub path: String,
    pub offset: u64,
    pub file_type: FileType,
    pub product_id: String,
}

impl DownloadUnit {
    pub(crate) fn from_product_bundles(bundles: Vec<ProductBundle>) -> Vec<DownloadUnit> {
        let download_units: Vec<DownloadUnit> = bundles
            .iter()
            .flat_map(|bundle| {
                bundle.product_files.iter().flat_map(move |depot_file| {
                    depot_file.to_download_units(bundle.product_id.clone())
                })
            })
            .collect::<Vec<DownloadUnit>>();
        download_units
    }
}
