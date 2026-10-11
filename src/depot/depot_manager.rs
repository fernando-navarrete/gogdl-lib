use std::{collections::HashMap, sync::Arc};

use tokio::sync::Mutex;

use crate::{
    client::HttpClient,
    depot::{
        build_metadata::BuildMetadata, depot_info::DepotInfo, error::DepotError,
        product_details::ProductDetails,
    },
};

#[derive(Clone)]
pub struct DepotManager {
    pub client: HttpClient,
    pub inner: Arc<Mutex<DepotManagerInner>>,
    /// Test-only: host standing in for `cdn.gog.com`'s depot manifests.
    #[cfg(test)]
    pub cdn_meta_base: String,
}

pub struct DepotManagerInner {
    pub product_details: HashMap<String, ProductDetails>,
    /// Unfiltered build metadata keyed by the build link. Never evicted, `clear_cache` keeps it.
    pub build_metadata: HashMap<String, BuildMetadata>,
    /// Depot manifests keyed by manifest hash. Can be tens of MB for a large game, so
    /// `DownloadManager::clear_cache` drops it.
    pub depot_info: HashMap<String, DepotInfo>,
}

impl DepotManager {
    pub fn new(client: HttpClient) -> Self {
        Self {
            client,
            inner: Arc::new(Mutex::new(DepotManagerInner {
                product_details: HashMap::new(),
                build_metadata: HashMap::new(),
                depot_info: HashMap::new(),
            })),
            #[cfg(test)]
            cdn_meta_base: "https://cdn.gog.com/content-system/v2/meta".to_string(),
        }
    }
    /// The URL of the depot manifest `depot_manifest` (its hash).
    pub fn depot_info_url(&self, depot_manifest: &str) -> String {
        #[cfg(test)]
        let base = self.cdn_meta_base.as_str();
        #[cfg(not(test))]
        let base = "https://cdn.gog.com/content-system/v2/meta";
        format!(
            "{base}/{}/{}/{depot_manifest}",
            &depot_manifest[0..2],
            &depot_manifest[2..4]
        )
    }
    pub async fn get_build_metadata(&self, game_link: &str) -> Result<BuildMetadata, DepotError> {
        BuildMetadata::get_build_metadata(self, game_link).await
    }
    pub async fn get_product_details(
        &self,
        product_id: &str,
    ) -> Result<ProductDetails, DepotError> {
        ProductDetails::get_product_details(self, product_id).await
    }
    pub async fn get_depot_info(&self, depot_manifest: &str) -> Result<DepotInfo, DepotError> {
        DepotInfo::get_depot_info(self, depot_manifest).await
    }
}
