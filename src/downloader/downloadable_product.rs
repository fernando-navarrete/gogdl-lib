use crate::{
    depot::Depot,
    downloader::{DownloadError, DownloadManager},
};

#[derive(Clone)]
pub struct DownloadableProduct {
    pub product_id: String,
    pub depots: Vec<Depot>,
}

impl DownloadableProduct {
    pub async fn get_downloadable_products(
        download_manager: &DownloadManager,
        game_id: i32,
        build_name: &str,
    ) -> Result<Vec<DownloadableProduct>, DownloadError> {
        {
            let lock = download_manager.inner.lock().await;
            if let Some(downloadable_products) = lock
                .downloadable_products
                .get(&(game_id, build_name.to_string()))
            {
                return Ok(downloadable_products.clone());
            }
        }
        let products = download_manager
            .resolve_build_depots(game_id, build_name)
            .await?;

        let owned_products = {
            let inner = download_manager.inner.lock().await;
            inner.games.get_owned_games().await?
        };

        let downloadable_products = products
            .into_iter()
            .filter(|(product_id, _)| owned_products.owned.contains(&product_id.parse().unwrap()))
            .map(|(product_id, depots)| DownloadableProduct { product_id, depots })
            .collect::<Vec<_>>();

        let mut lock = download_manager.inner.lock().await;
        lock.downloadable_products.insert(
            (game_id, build_name.to_string()),
            downloadable_products.clone(),
        );
        Ok(downloadable_products)
    }
}
