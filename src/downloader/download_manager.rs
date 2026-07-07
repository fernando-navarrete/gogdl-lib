use std::{collections::HashMap, sync::Arc};

use tokio::sync::{Mutex, mpsc};

use crate::{
    RepairEvent, VerifyEvent,
    auth::AuthManager,
    client::HttpClient,
    depot::DepotManager,
    downloader::{
        downloadable_files::DownloadableFiles, downloadable_product::DownloadableProduct,
        downloader::Downloader, error::DownloadError,
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
    pub downloader: HashMap<i32, Downloader>,
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
                downloader: HashMap::new(),
            })),
            client,
        }
    }
    pub async fn verify_download(
        &self,
        path: &str,
        files: Vec<DownloadableFiles>,
        tx: mpsc::UnboundedSender<VerifyEvent>,
    ) -> Result<(), DownloadError> {
        let (auth, links) = {
            let inner = self.inner.lock().await;
            let auth = inner.auth.clone();
            let links = inner.secure_links.clone();
            (auth, links)
        };
        let downloader = Downloader::new(self.client.clone(), links, auth).await;
        downloader.verify(files, path, tx).await?;
        Ok(())
    }
    pub async fn repair_download(
        &self,
        path: &str,
        files: Vec<DownloadableFiles>,
        tx: mpsc::UnboundedSender<RepairEvent>,
    ) -> Result<(), DownloadError> {
        let (auth, links) = {
            let inner = self.inner.lock().await;
            let auth = inner.auth.clone();
            let links = inner.secure_links.clone();
            (auth, links)
        };
        let downloader = Downloader::new(self.client.clone(), links, auth).await;
        downloader.repair_download(files, path, tx).await?;
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
