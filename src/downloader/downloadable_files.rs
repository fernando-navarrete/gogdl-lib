use std::collections::HashMap;

use crate::{
    depot::{Depot, DepotFile},
    downloader::{DownloadError, DownloadManager},
};

pub struct DownloadableFiles {
    pub product_id: String,
    pub product_files: Vec<DepotFile>,
}

impl DownloadableFiles {
    pub async fn get_download_files(
        download_manager: &DownloadManager,
        game_id: i32,
        build_name: &str,
        selected_products: &[&str],
    ) -> Result<Vec<DownloadableFiles>, DownloadError> {
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
        let products = {
            let mut products: HashMap<&str, Vec<Depot>> = HashMap::new();

            for depot in depots.iter().cloned() {
                products
                    .entry(&depot.product_id)
                    .or_insert_with(Vec::new)
                    .push(depot.clone());
            }

            let products_vec = products.into_iter().collect::<Vec<_>>();
            products_vec
        };

        let filtered_products = products
            .iter()
            .filter(|(product_id, _)| selected_products.contains(&product_id))
            .collect::<Vec<_>>();

        let mut downloadable_files: Vec<DownloadableFiles> = Vec::new();

        for (id, depots) in filtered_products {
            for depot in depots {
                let depot_info = {
                    let inner = download_manager.inner.lock().await;
                    inner.depot.get_depot_info(&depot.manifest).await?
                };

                let depot_files = depot_info.depot.items;
                downloadable_files.push(DownloadableFiles {
                    product_id: id.to_string(),
                    product_files: depot_files,
                });
            }
        }

        Ok(downloadable_files)
    }
}
