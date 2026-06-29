use std::sync::Arc;

use tokio::sync::Mutex;

use crate::{
    auth::AuthManager,
    client::HttpClient,
    depot::DepotManager,
    downloader::{
        downloadable_files::DownloadableFiles, downloadable_product::DownloadableProduct,
        error::DownloadError,
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
    ) -> Result<Vec<DownloadableFiles>, DownloadError> {
        DownloadableFiles::get_download_files(self, game_id, build_name, selected_products).await
    }
}
