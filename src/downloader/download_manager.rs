use std::{collections::HashMap, sync::Arc};

use tokio::sync::{Mutex, mpsc};

use crate::{
    DownloadJobEvent, RepairEvent, VerifyEvent,
    auth::AuthManager,
    client::HttpClient,
    depot::DepotManager,
    downloader::{
        DownloadConfig, control::DownloadControl, downloadable_files::DownloadableFiles,
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
    pub async fn verify_download(
        &self,
        path: &str,
        files: Vec<DownloadableFiles>,
        tx: mpsc::UnboundedSender<VerifyEvent>,
    ) -> Result<(), DownloadError> {
        let (auth, links, config) = {
            let inner = self.inner.lock().await;
            let auth = inner.auth.clone();
            let links = inner.secure_links.clone();
            let config = inner.download_config.clone();
            (auth, links, config)
        };
        let downloader = Downloader::new_with_config(self.client.clone(), links, auth, config);
        downloader.verify(files, path, tx).await?;
        Ok(())
    }
    pub async fn repair_download(
        &self,
        path: &str,
        files: Vec<DownloadableFiles>,
        control: DownloadControl,
        tx: mpsc::UnboundedSender<RepairEvent>,
    ) -> Result<(), DownloadError> {
        let (auth, links, config) = {
            let inner = self.inner.lock().await;
            let auth = inner.auth.clone();
            let links = inner.secure_links.clone();
            let config = inner.download_config.clone();
            (auth, links, config)
        };
        let downloader = Downloader::new_with_config(self.client.clone(), links, auth, config);
        downloader.repair_download(files, path, control, tx).await?;
        Ok(())
    }
    pub async fn download(
        &self,
        path: &str,
        files: Vec<DownloadableFiles>,
        control: DownloadControl,
        tx: mpsc::UnboundedSender<DownloadJobEvent>,
    ) -> Result<(), DownloadError> {
        let (auth, links, config) = {
            let inner = self.inner.lock().await;
            let auth = inner.auth.clone();
            let links = inner.secure_links.clone();
            let config = inner.download_config.clone();
            (auth, links, config)
        };
        let downloader = Downloader::new_with_config(self.client.clone(), links, auth, config);
        downloader.download(files, path, control, tx).await?;
        Ok(())
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
