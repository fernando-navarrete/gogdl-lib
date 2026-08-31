use std::collections::HashMap;

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
        let game_builds = {
            let inner = download_manager.inner.lock().await;
            inner.games.get_game_builds(game_id).await?
        };
        let build = game_builds
            .items
            .iter()
            .find(|b| b.version_name == build_name)
            .ok_or(DownloadError::BuildNotFound)?;

        let build_metadata = {
            let inner = download_manager.inner.lock().await;
            inner.depot.get_build_metadata(&build.link).await?
        };

        let depots = build_metadata.depots.iter().collect::<Vec<_>>();
        let mut products: HashMap<&str, Vec<Depot>> = HashMap::new();

        for depot in depots.iter().cloned() {
            products
                .entry(&depot.product_id)
                .or_insert_with(Vec::new)
                .push(depot.clone());
        }

        let owned_products = {
            let inner = download_manager.inner.lock().await;
            inner.games.get_owned_games().await?
        };

        let downloadable_products = products
            .into_iter()
            .filter(|(product_id, _)| {
                let product_id_i32 = match product_id.parse::<i32>() {
                    Ok(id) => id,
                    Err(_err) => {
                        return false;
                    }
                };
                owned_products.owned.contains(&product_id_i32)
            })
            .map(|(product_id, depots)| DownloadableProduct {
                product_id: product_id.to_string(),
                depots,
            })
            .collect::<Vec<_>>();

        let mut lock = download_manager.inner.lock().await;
        lock.downloadable_products.insert(
            (game_id, build_name.to_string()),
            downloadable_products.clone(),
        );
        Ok(downloadable_products)
    }
}
