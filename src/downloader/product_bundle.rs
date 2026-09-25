use std::collections::HashMap;

use crate::{
    depot::{Depot, DepotFile},
    downloader::{DownloadError, DownloadManager},
};

/// One product's resolved depot manifest — the unit
/// [`GogDl::verify_files`](crate::GogDl::verify_files),
/// [`GogDl::download_game`](crate::GogDl::download_game) and
/// [`GogDl::repair_game`](crate::GogDl::repair_game) consume. Produced by
/// [`GogDl::get_product_bundles`](crate::GogDl::get_product_bundles).
///
/// Not [`Clone`] — hold onto the `Vec<ProductBundle>` you get back if you
/// need to run more than one operation against it, since each of the three
/// methods above takes it by value.
pub struct ProductBundle {
    /// The product's ID, as a string.
    pub product_id: String,
    /// Every file this product's depot manifest lists. `DepotFile` is not
    /// exported from this crate — you can hold, index and pass along this
    /// `Vec`, but cannot name the element type in your own signatures.
    pub product_files: Vec<DepotFile>,
}

impl ProductBundle {
    /// Not reachable from outside the crate — `DownloadManager` is not
    /// exported. Call
    /// [`GogDl::get_product_bundles`](crate::GogDl::get_product_bundles)
    /// instead, which delegates here internally.
    pub async fn get_download_files(
        download_manager: &DownloadManager,
        game_id: i32,
        build_name: &str,
        selected_products: &[i32],
    ) -> Result<Vec<ProductBundle>, DownloadError> {
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
                    .or_default()
                    .push(depot.clone());
            }

            products.into_iter().collect::<Vec<_>>()
        };

        let filtered_products = products
            .iter()
            .filter(|(product_id, _)| {
                let product_id_i32 = match product_id.parse::<i32>() {
                    Ok(id) => id,
                    Err(_) => return false,
                };
                selected_products.contains(&product_id_i32)
            })
            .collect::<Vec<_>>();

        let mut downloadable_files: Vec<ProductBundle> = Vec::new();

        for (id, depots) in filtered_products {
            for depot in depots {
                let depot_info = {
                    let inner = download_manager.inner.lock().await;
                    inner.depot.get_depot_info(&depot.manifest).await?
                };

                let depot_files = depot_info.depot.items;
                downloadable_files.push(ProductBundle {
                    product_id: id.to_string(),
                    product_files: depot_files,
                });
            }
        }

        Ok(downloadable_files)
    }
}
