use std::sync::Arc;

use tokio::sync::Mutex;

use crate::{
    auth::AuthManager,
    client::HttpClient,
    downloader::{build_metadata::BuildMetadata, error::DownloadError},
};

pub struct DownloadManager {
    pub client: HttpClient,
    pub inner: Arc<Mutex<DownloadManagerInner>>,
}

pub struct DownloadManagerInner {
    pub auth: AuthManager,
}

impl DownloadManager {
    pub fn new(client: HttpClient, auth: AuthManager) -> Self {
        Self {
            client,
            inner: Arc::new(Mutex::new(DownloadManagerInner { auth })),
        }
    }
    pub async fn get_build_metadata(
        &self,
        game_link: &str,
    ) -> Result<BuildMetadata, DownloadError> {
        BuildMetadata::get_build_metadata(self, game_link).await
    }
}
