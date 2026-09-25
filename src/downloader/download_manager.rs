use std::{collections::HashMap, sync::Arc};

use tokio::sync::{Mutex, mpsc};

use crate::{
    client::HttpClient,
    depot::DepotManager,
    downloader::{
        DownloadStageEvent, ProductBundle, downloadable_product::DownloadableProduct,
        engine::Downloader, error::DownloadError, progress_reporting::VerificationEvent,
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
    pub depot: DepotManager,
    pub secure_links: SecureLinksManager,
    pub games: GamesManager,
    pub downloadable_products: HashMap<(i32, String), Vec<DownloadableProduct>>,
}

impl DownloadManager {
    pub fn new(
        depot: DepotManager,
        secure_links: SecureLinksManager,
        games: GamesManager,
        client: HttpClient,
    ) -> Self {
        Self {
            inner: Arc::new(Mutex::new(DownloadManagerInner {
                depot,
                secure_links,
                games,
                downloadable_products: HashMap::new(),
            })),
            client,
        }
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
        selected_products: &[i32],
    ) -> Result<Vec<ProductBundle>, DownloadError> {
        ProductBundle::get_download_files(self, game_id, build_name, selected_products).await
    }
    pub async fn verify_download(
        &self,
        bundles: Vec<ProductBundle>,
        path: &str,
        tx: mpsc::UnboundedSender<VerificationEvent>,
    ) -> Result<(), DownloadError> {
        let links = {
            let inner = self.inner.lock().await;

            inner.secure_links.clone()
        };
        let downloader = Downloader::new(self.client.clone(), links);
        downloader.verify(bundles, path, tx).await?;
        Ok(())
    }
    pub async fn download_game(
        &self,
        bundles: Vec<ProductBundle>,
        path: &str,
        tx: mpsc::UnboundedSender<DownloadStageEvent>,
    ) -> Result<(), DownloadError> {
        let links = {
            let inner = self.inner.lock().await;

            inner.secure_links.clone()
        };
        let downloader = Downloader::new(self.client.clone(), links);
        downloader.download(bundles, path, tx).await?;
        Ok(())
    }
    pub async fn repair_game(
        &self,
        bundles: Vec<ProductBundle>,
        path: &str,
        tx: mpsc::UnboundedSender<DownloadStageEvent>,
    ) -> Result<(), DownloadError> {
        let links = {
            let inner = self.inner.lock().await;

            inner.secure_links.clone()
        };
        let downloader = Downloader::new(self.client.clone(), links);
        downloader.repair(bundles, path, tx).await?;
        Ok(())
    }
}
