use std::path::Path;

use tokio::sync::mpsc::UnboundedSender;

use crate::client::HttpClient;
use crate::proton::{
    error::ProtonError,
    release::{ProtonProgress, Release},
};

/// Unlike the other managers in this crate, `ProtonManager` holds no
/// `Arc<Mutex<Inner>>` cache and no `AuthManager`: GitHub's releases API is
/// public (no auth needed), and the release list genuinely changes over time
/// as new Proton-GE builds ship, so caching it indefinitely would serve
/// callers stale versions rather than the cheap-to-refetch data that
/// `remote_config`-style caches are meant for.
#[derive(Clone)]
pub struct ProtonManager {
    pub client: HttpClient,
}

impl ProtonManager {
    pub fn new(client: HttpClient) -> Self {
        Self { client }
    }
    /// Lists Proton-GE releases from GitHub, one page at a time (GitHub's own
    /// pagination; `page` is 1-indexed).
    pub async fn get_releases(&self, page: i32) -> Result<Vec<Release>, ProtonError> {
        Release::get_releases(self, page).await
    }
    /// Downloads and extracts `release`'s Proton-GE tarball into `path`,
    /// verifying its SHA-256 checksum first if GitHub provided one.
    pub async fn download_release(
        &self,
        release: &Release,
        path: &Path,
        tx: UnboundedSender<ProtonProgress>,
    ) -> Result<(), ProtonError> {
        release.download(self, path, tx).await
    }
}
