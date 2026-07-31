use std::{collections::HashMap, sync::Arc};

use tokio::sync::{Mutex, mpsc};

use crate::{
    DownloadJobEvent, RepairEvent, VerifyEvent,
    auth::AuthManager,
    client::HttpClient,
    depot::{Depot, DepotManager},
    downloader::{
        DownloadConfig, downloadable_files::DownloadableFiles,
        downloadable_product::DownloadableProduct, downloader::Downloader, error::DownloadError,
    },
    games::GamesManager,
    secure_links::SecureLinksManager,
};

#[derive(Clone)]
pub struct DownloadManager {
    pub inner: Arc<Mutex<DownloadManagerInner>>,
    pub client: HttpClient,
}

pub struct DownloadManagerInner {
    pub auth: AuthManager,
    pub depot: DepotManager,
    pub secure_links: SecureLinksManager,
    pub games: GamesManager,
    pub downloadable_products: HashMap<(i32, String), Vec<DownloadableProduct>>,
    pub downloadable_files: HashMap<(i32, String, Vec<String>), Vec<DownloadableFiles>>,
    /// Network tuning (concurrency bounds, timeouts, retries) applied to
    /// every `Downloader` this manager creates. Defaults are tuned for
    /// network-bound throughput; see `DownloadConfig`.
    pub download_config: DownloadConfig,
}

impl DownloadManager {
    pub fn new(
        auth: AuthManager,
        depot: DepotManager,
        secure_links: SecureLinksManager,
        games: GamesManager,
        client: HttpClient,
    ) -> Self {
        Self {
            inner: Arc::new(Mutex::new(DownloadManagerInner {
                auth,
                depot,
                secure_links,
                games,
                downloadable_products: HashMap::new(),
                downloadable_files: HashMap::new(),
                download_config: DownloadConfig::default(),
            })),
            client,
        }
    }
    /// Overrides the network tuning used by subsequent `download`,
    /// `repair_download`, and `verify` calls. Useful for a consumer that
    /// wants to raise/lower the concurrency ceiling for a known-slow or
    /// known-fast connection, or shorten timeouts for a flaky one.
    pub async fn set_download_config(&self, config: DownloadConfig) {
        let mut inner = self.inner.lock().await;
        inner.download_config = config;
    }
    /// Snapshots the manager's current auth/secure-links/config under the
    /// lock and builds the per-job `Downloader` from them — the shared
    /// preamble of `verify_download`/`repair_download`/`download`.
    async fn make_downloader(&self) -> Downloader {
        let (auth, links, config) = {
            let inner = self.inner.lock().await;
            (
                inner.auth.clone(),
                inner.secure_links.clone(),
                inner.download_config.clone(),
            )
        };
        Downloader::new_with_config(self.client.clone(), links, auth, config)
    }
    pub async fn verify_download(
        &self,
        path: &str,
        files: Vec<DownloadableFiles>,
        tx: mpsc::UnboundedSender<VerifyEvent>,
    ) -> Result<(), DownloadError> {
        self.make_downloader().await.verify(files, path, tx).await?;
        Ok(())
    }
    pub async fn repair_download(
        &self,
        path: &str,
        files: Vec<DownloadableFiles>,
        tx: mpsc::UnboundedSender<RepairEvent>,
    ) -> Result<(), DownloadError> {
        self.make_downloader()
            .await
            .repair_download(files, path, tx)
            .await?;
        Ok(())
    }
    pub async fn download(
        &self,
        path: &str,
        files: Vec<DownloadableFiles>,
        tx: mpsc::UnboundedSender<DownloadJobEvent>,
    ) -> Result<(), DownloadError> {
        self.make_downloader()
            .await
            .download(files, path, tx)
            .await?;
        Ok(())
    }
    /// Resolves `build_name` among `game_id`'s builds and groups the build's
    /// depots by product id — the shared first half of
    /// `get_downloadable_products` and `get_downloadable_files`.
    pub(crate) async fn resolve_build_depots(
        &self,
        game_id: i32,
        build_name: &str,
    ) -> Result<HashMap<String, Vec<Depot>>, DownloadError> {
        let game_builds = {
            let inner = self.inner.lock().await;
            inner.games.get_game_builds(game_id).await?
        };
        let build = game_builds
            .items
            .iter()
            .find(|b| b.version_name == build_name)
            .ok_or(DownloadError::BuildNotFound)?;

        let build_metadata = {
            let inner = self.inner.lock().await;
            inner.depot.get_build_metadata(&build.link).await?
        };

        let mut products: HashMap<String, Vec<Depot>> = HashMap::new();
        for depot in build_metadata.depots {
            products
                .entry(depot.product_id.clone())
                .or_default()
                .push(depot);
        }
        Ok(products)
    }
    pub async fn get_downloadable_products(
        &self,
        game_id: i32,
        build_name: &str,
    ) -> Result<Vec<DownloadableProduct>, DownloadError> {
        DownloadableProduct::get_downloadable_products(self, game_id, build_name).await
    }
    pub async fn get_downloadable_files(
        &self,
        game_id: i32,
        build_name: &str,
        selected_products: &[&str],
    ) -> Result<Vec<DownloadableFiles>, DownloadError> {
        DownloadableFiles::get_download_files(self, game_id, build_name, selected_products).await
    }
}
