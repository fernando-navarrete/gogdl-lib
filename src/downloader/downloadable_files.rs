use crate::{
    depot::DepotFile,
    downloader::{DownloadError, DownloadManager},
};

#[derive(Clone)]
pub struct DownloadableFiles {
    pub product_id: String,
    pub product_files: Vec<DepotFile>,
    /// Whether these files come from a dependency/redistributable depot
    /// (DirectX, VC++ runtimes, ...) rather than a game depot. Dependency
    /// content is hosted under the CDN's `dependencies/store` path, so only
    /// units from a dependency depot should ever fall back to
    /// `parse_url_redist` on a primary download failure — see
    /// `Downloader::download_chunk`. Every current caller of
    /// `get_download_files` resolves game depots, so this is always `false`
    /// today; it exists for a future dependency-depot flow.
    pub is_dependency: bool,
}

impl DownloadableFiles {
    pub async fn get_download_files(
        download_manager: &DownloadManager,
        game_id: i32,
        build_name: &str,
        selected_products: &[&str],
    ) -> Result<Vec<DownloadableFiles>, DownloadError> {
        let cache_key = (
            game_id,
            build_name.to_string(),
            selected_products
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>(),
        );
        {
            let lock = download_manager.inner.lock().await;
            if let Some(downloadable_files) = lock.downloadable_files.get(&cache_key) {
                return Ok(downloadable_files.clone());
            }
        }

        let products = download_manager
            .resolve_build_depots(game_id, build_name)
            .await?;

        let filtered_products = products
            .into_iter()
            .filter(|(product_id, _)| selected_products.contains(&product_id.as_str()))
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
                    is_dependency: false,
                });
            }
        }

        let mut lock = download_manager.inner.lock().await;
        lock.downloadable_files
            .insert(cache_key, downloadable_files.clone());
        Ok(downloadable_files)
    }
}
