use std::sync::Arc;

use tokio::sync::Mutex;

use crate::{
    auth::AuthManager,
    client::HttpClient,
    depot::{
        build_metadata::BuildMetadata, depot_info::DepotInfo, error::DepotError,
        product_details::ProductDetails,
    },
};

pub struct DepotManager {
    pub client: HttpClient,
    pub inner: Arc<Mutex<DepotManagerInner>>,
}

pub struct DepotManagerInner {
    pub auth: AuthManager,
}

impl DepotManager {
    pub fn new(client: HttpClient, auth: AuthManager) -> Self {
        Self {
            client,
            inner: Arc::new(Mutex::new(DepotManagerInner { auth })),
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
