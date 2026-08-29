use std::{path::PathBuf, sync::Arc};

use futures_util::{StreamExt, stream};

use crate::{
    auth::AuthManager,
    client::HttpClient,
    downloader::{
        DownloadError, DownloadUnit, PathResolver, ProductBundle,
        util::{ChecksumAlgorithm, compute_chunk_checksum},
    },
    secure_links::SecureLinksManager,
};

pub struct Downloader {
    pub client: HttpClient,
    pub secure_links: SecureLinksManager,
    pub auth: AuthManager,
}

impl Downloader {
    pub fn new(client: HttpClient, secure_links: SecureLinksManager, auth: AuthManager) -> Self {
        Self {
            client,
            secure_links,
            auth,
        }
    }

    pub async fn verify(
        &self,
        bundles: Vec<ProductBundle>,
        path: &str,
    ) -> Result<(), DownloadError> {
        let path_resolver = Arc::new(PathResolver::new(PathBuf::from(path)).await?);

        let download_units = bundles
            .iter()
            .flat_map(|bundle| bundle.product_files.clone())
            .flat_map(|depot_file| DownloadUnit::from_depot_file(depot_file))
            .collect::<Vec<DownloadUnit>>();

        let threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1);

        stream::iter(download_units)
            .map(|download_unit| {
                let path_resolver = path_resolver.clone();
                async move {
                    let opt_path = match path_resolver
                        .resolve_existing_path(&download_unit.path)
                        .await
                    {
                        Ok(path) => path,
                        Err(e) => {
                            println!("ERROR: {}: {}", download_unit.path, e);
                            return Some(download_unit);
                        }
                    };

                    let final_path = match opt_path {
                        Some(path) => path,
                        None => {
                            println!("ERROR: {}: FILE_NOT_FOUND", download_unit.path);
                            return Some(download_unit);
                        }
                    };

                    let hashing_algorithm = ChecksumAlgorithm::Md5;

                    let actual_checksum = match compute_chunk_checksum(
                        final_path,
                        download_unit.offset,
                        download_unit.size,
                        hashing_algorithm,
                    )
                    .await
                    {
                        Ok(checksum) => checksum,
                        Err(err) => {
                            println!("ERROR: {}: {}", download_unit.path, err);
                            return Some(download_unit);
                        }
                    };

                    if actual_checksum != download_unit.md5 {
                        println!("ERROR: {} CHECKSUM MISMATCH", download_unit.path);
                        return Some(download_unit);
                    }

                    return None;
                }
            })
            .buffer_unordered(threads)
            .collect::<Vec<_>>()
            .await;

        Ok(())
    }
}
