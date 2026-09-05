use std::sync::Arc;

use tokio::sync::Mutex;

use crate::{
    client::HttpClient,
    proton::{error::ProtonError, proton_ge_releases_page::ProtonGeReleasesPage},
};

#[derive(Clone)]
pub struct ProtonManager {
    inner: Arc<Mutex<ProtonManagerInner>>,
    pub client: HttpClient,
}

pub struct ProtonManagerInner {}

impl ProtonManager {
    pub fn new(client: HttpClient) -> Self {
        Self {
            inner: Arc::new(Mutex::new(ProtonManagerInner {})),
            client,
        }
    }
    pub async fn get_releases_page(
        &self,
        page: u32,
        per_page: u32,
    ) -> Result<ProtonGeReleasesPage, ProtonError> {
        ProtonGeReleasesPage::get_releases_page(page, per_page, self).await
    }
}
