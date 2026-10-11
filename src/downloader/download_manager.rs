use std::{collections::HashMap, sync::Arc};

use tokio::sync::{Mutex, mpsc};

use crate::{
    client::HttpClient,
    depot::{BuildMetadata, DepotManager},
    downloader::{
        DownloadStageEvent, ProductBundle,
        downloadable_product::DownloadableProduct,
        engine::Downloader,
        error::DownloadError,
        product_removal::{ProductRemoval, remove_files},
        product_size::{ProductSize, sum_by_product},
        progress_reporting::VerificationEvent,
    },
    games::GamesManager,
    secure_links::SecureLinksManager,
};

#[derive(Clone)]
pub struct DownloadManager {
    pub inner: Arc<Mutex<DownloadManagerInner>>,
    pub client: HttpClient,
    // Shared handles kept outside the mutex so none is held across a fetch.
    pub depot: DepotManager,
    pub secure_links: SecureLinksManager,
    pub games: GamesManager,
}

pub struct DownloadManagerInner {
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
                downloadable_products: HashMap::new(),
            })),
            client,
            depot,
            secure_links,
            games,
        }
    }
    /// Drops the downloadable products.
    pub async fn clear_cache(&self) {
        self.inner.lock().await.downloadable_products.clear();
    }
    /// The language-filtered metadata of `build_name`, the one place a build is looked up by name.
    pub async fn get_build_metadata(
        &self,
        game_id: i32,
        build_name: &str,
    ) -> Result<BuildMetadata, DownloadError> {
        let game_builds = self.games.get_game_builds(game_id).await?;
        let build = game_builds
            .items
            .iter()
            .find(|b| b.version_name == build_name)
            .ok_or(DownloadError::BuildNotFound)?;

        Ok(self.depot.get_build_metadata(&build.link).await?)
    }
    pub async fn get_product_sizes(
        &self,
        game_id: i32,
        build_name: &str,
    ) -> Result<Vec<ProductSize>, DownloadError> {
        let build_metadata = self.get_build_metadata(game_id, build_name).await?;
        Ok(sum_by_product(&build_metadata.depots))
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
        let downloader = Downloader::new(self.client.clone(), self.secure_links.clone());
        downloader.verify(bundles, path, tx).await?;
        Ok(())
    }
    pub async fn download_game(
        &self,
        bundles: Vec<ProductBundle>,
        path: &str,
        tx: mpsc::UnboundedSender<DownloadStageEvent>,
    ) -> Result<(), DownloadError> {
        let downloader = Downloader::new(self.client.clone(), self.secure_links.clone());
        downloader.download(bundles, path, tx).await?;
        Ok(())
    }
    pub async fn repair_game(
        &self,
        bundles: Vec<ProductBundle>,
        path: &str,
        tx: mpsc::UnboundedSender<DownloadStageEvent>,
    ) -> Result<(), DownloadError> {
        let downloader = Downloader::new(self.client.clone(), self.secure_links.clone());
        downloader.repair(bundles, path, tx).await?;
        Ok(())
    }
    /// Deletes the files only `remove`'s manifests list. An id in both lists is kept, and an empty
    /// `remove` returns before anything is fetched.
    pub async fn remove_products(
        &self,
        game_id: i32,
        path: &str,
        build_name: &str,
        keep: &[i32],
        remove: &[i32],
    ) -> Result<ProductRemoval, DownloadError> {
        let remove: Vec<i32> = remove
            .iter()
            .copied()
            .filter(|id| !keep.contains(id))
            .collect();
        if remove.is_empty() {
            return Ok(ProductRemoval::default());
        }
        let keep = self
            .get_downloadable_files(game_id, build_name, keep)
            .await?;
        let remove = self
            .get_downloadable_files(game_id, build_name, &remove)
            .await?;
        remove_files(path, &keep, &remove).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{client::HttpClient, depot::DepotManager};

    /// No auth and no network here, so any manifest or build lookup would fail: `Ok` proves an
    /// empty `remove`, or one the `keep` list covers, fetched nothing.
    #[tokio::test]
    async fn nothing_to_remove_fetches_nothing() {
        let client = HttpClient::new_with_client(reqwest::Client::new());
        let games = GamesManager::new(client.clone());
        let manager = DownloadManager::new(
            DepotManager::new(client.clone()),
            SecureLinksManager::new(client.clone(), games.clone()),
            games,
            client,
        );
        let tmp = crate::test_support::TempDir::new();
        let path = tmp.path().to_str().unwrap();
        for (keep, remove) in [(&[1][..], &[][..]), (&[1][..], &[1][..])] {
            let removal = manager
                .remove_products(7, path, "b", keep, remove)
                .await
                .unwrap();
            assert_eq!(removal, ProductRemoval::default());
        }
    }
}
