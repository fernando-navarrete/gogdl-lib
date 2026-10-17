use std::collections::HashMap;

use crate::{
    depot::Depot,
    downloader::{DownloadError, DownloadManager},
};

/// One owned, downloadable sub-product (base game, a DLC, ...) of a build,
/// as returned by
/// [`GogDl::get_downloadable_products`](crate::GogDl::get_downloadable_products).
#[derive(Clone)]
pub struct DownloadableProduct {
    /// The product's ID, as a string — pass it in `selected_products` to
    /// [`GogDl::get_product_bundles`](crate::GogDl::get_product_bundles).
    pub product_id: String,
    /// The product's raw depots. `Depot` is not exported from this crate, so
    /// it can only be held opaquely (e.g. `.len()`, passing the `Vec` back
    /// through), not named in a signature or `let` binding. For sizes, use
    /// [`GogDl::get_product_sizes`](crate::GogDl::get_product_sizes).
    pub depots: Vec<Depot>,
}

impl DownloadableProduct {
    /// Call
    /// [`GogDl::get_downloadable_products`](crate::GogDl::get_downloadable_products)
    /// instead, which delegates here internally.
    pub(crate) async fn get_downloadable_products(
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
        let build_metadata = download_manager
            .get_build_metadata(game_id, build_name)
            .await?;

        let depots = build_metadata.depots.iter().collect::<Vec<_>>();
        let mut products: HashMap<&str, Vec<Depot>> = HashMap::new();

        for depot in depots.iter().cloned() {
            products
                .entry(&depot.product_id)
                .or_default()
                .push(depot.clone());
        }

        let owned_products = download_manager.games.get_owned_products().await?;

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

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Duration};

    use super::*;
    use crate::{
        client::HttpClient, depot::DepotManager, games::GamesManager,
        secure_links::SecureLinksManager,
    };

    /// A cold lookup stuck inside the games manager (its `inner` is held here,
    /// the await the manager's own lock used to span) must not block a cached
    /// lookup for another game.
    #[tokio::test(start_paused = true)]
    async fn a_cold_lookup_does_not_block_a_cached_one() {
        let client = HttpClient::new_with_client(reqwest::Client::new());
        let games = GamesManager::new(client.clone());
        let manager = Arc::new(DownloadManager::new(
            DepotManager::new(client.clone()),
            SecureLinksManager::new(client.clone(), games.clone()),
            games.clone(),
            client,
        ));
        manager.inner.lock().await.downloadable_products.insert(
            (2, "b".to_string()),
            vec![DownloadableProduct {
                product_id: "2".into(),
                depots: Vec::new(),
            }],
        );

        let games_guard = games.inner.lock().await;
        let cold = tokio::spawn({
            let manager = manager.clone();
            async move { manager.get_downloadable_products(1, "a").await }
        });
        tokio::time::sleep(Duration::from_millis(1)).await;

        let cached = tokio::time::timeout(
            Duration::from_secs(1),
            manager.get_downloadable_products(2, "b"),
        )
        .await
        .expect("a cached lookup waited on another game's fetch")
        .unwrap();
        assert_eq!(cached.len(), 1);

        drop(games_guard);
        cold.abort();
    }

    #[tokio::test]
    async fn clear_cache_drops_the_downloadable_products() {
        let client = HttpClient::new_with_client(reqwest::Client::new());
        let games = GamesManager::new(client.clone());
        let manager = DownloadManager::new(
            DepotManager::new(client.clone()),
            SecureLinksManager::new(client.clone(), games.clone()),
            games,
            client,
        );
        manager
            .inner
            .lock()
            .await
            .downloadable_products
            .insert((2, "b".to_string()), Vec::new());

        manager.clear_cache().await;
        assert!(manager.inner.lock().await.downloadable_products.is_empty());
    }
}
