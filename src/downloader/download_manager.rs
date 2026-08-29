use std::{collections::HashMap, sync::Arc};

use tokio::sync::{Mutex, mpsc};

use crate::{
    auth::AuthManager,
    client::HttpClient,
    depot::DepotManager,
    downloader::{
        ProductBundle, downloadable_product::DownloadableProduct, downloader::Downloader,
        error::DownloadError, progress_reporting::VerificationEvent,
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
        let (auth, links) = {
            let inner = self.inner.lock().await;
            let auth = inner.auth.clone();
            let links = inner.secure_links.clone();
            (auth, links)
        };
        let downloader = Downloader::new(self.client.clone(), links, auth);
        downloader.verify(bundles, path, tx).await?;
        Ok(())
    }
}
