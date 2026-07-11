use std::{collections::HashMap, sync::Arc};

use tokio::sync::Mutex;

use crate::{
    auth::AuthManager,
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
}

pub struct DepotManagerInner {
    pub auth: AuthManager,
    pub product_details: HashMap<String, ProductDetails>,
    /// Keyed by the build's manifest link. Safe to cache indefinitely:
    /// each build's link is a distinct, immutable manifest.
    pub build_metadata: HashMap<String, BuildMetadata>,
    /// Keyed by depot manifest hash. Safe to cache indefinitely: depot
    /// manifests are content-addressed and never change under one hash.
    pub depot_info: HashMap<String, DepotInfo>,
}

impl DepotManager {
    pub fn new(client: HttpClient, auth: AuthManager) -> Self {
        Self {
            client,
            inner: Arc::new(Mutex::new(DepotManagerInner {
                auth,
                product_details: HashMap::new(),
                build_metadata: HashMap::new(),
                depot_info: HashMap::new(),
            })),
        }
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
