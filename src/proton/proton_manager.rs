use std::{path::Path, sync::Arc};

use tokio::sync::{Mutex, mpsc};

use crate::{
    ProtonGeRelease,
    client::HttpClient,
    proton::{
        ProtonDownloadEvent, ProtonDownloader, error::ProtonError,
        proton_ge_releases_page::ProtonGeReleasesPage,
    },
};

/// Crate-internal manager backing
/// [`GogDl::get_proton_releases`](crate::GogDl::get_proton_releases). Not
/// exported — `GogDl` is the only entry point, mirroring `GamesManager` and
/// `DepotManager`.
#[derive(Clone)]
pub struct ProtonManager {
    inner: Arc<Mutex<ProtonManagerInner>>,
    pub client: HttpClient,
}

/// Placeholder for future in-process caching of release pages, mirroring
/// the other managers' `inner` state. Currently empty and unused — every
/// call to [`ProtonManager::get_releases_page`] re-fetches from GitHub.
pub struct ProtonManagerInner {}

impl ProtonManager {
    /// Builds a `ProtonManager` around an already-constructed [`HttpClient`].
    pub fn new(client: HttpClient) -> Self {
        Self {
            inner: Arc::new(Mutex::new(ProtonManagerInner {})),
            client,
        }
    }
    /// Fetches one page of Proton-GE releases from GitHub. See
    /// [`GogDl::get_proton_releases`](crate::GogDl::get_proton_releases) for
    /// the public-facing contract (pagination, auth requirements, caching).
    pub async fn get_releases_page(
        &self,
        page: u32,
        per_page: u32,
    ) -> Result<ProtonGeReleasesPage, ProtonError> {
        ProtonGeReleasesPage::get_releases_page(page, per_page, self).await
    }
    /// Downloads and extracts one Proton-GE release. See
    /// [`GogDl::download_proton_release`](crate::GogDl::download_proton_release)
    /// for the public-facing contract.
    pub async fn download_proton_release(
        &self,
        release: &ProtonGeRelease,
        path: &Path,
        tx: mpsc::UnboundedSender<ProtonDownloadEvent>,
    ) -> Result<std::path::PathBuf, ProtonError> {
        let downloader = ProtonDownloader::new(self.client.clone());
        downloader.download_proton_release(release, path, tx).await
    }
    /// Fetches a single Proton-GE release by its tag. See
    /// [`GogDl::get_proton_release_by_tag`](crate::GogDl::get_proton_release_by_tag)
    /// for the public-facing contract.
    pub async fn get_release_by_tag(&self, tag: &str) -> Result<ProtonGeRelease, ProtonError> {
        ProtonGeRelease::get_by_tag(tag, self).await
    }
}
