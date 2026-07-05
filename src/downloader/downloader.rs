use std::{path::PathBuf, sync::Arc, thread};

use crate::{
    DownloadableFiles,
    client::HttpClient,
    downloader::{
        DownloadError,
        util::{ChecksumAlgorithm, PathResolver, compute_checksum},
    },
    secure_links::SecureLinksManager,
};
use futures::{StreamExt, stream};
use tokio::sync::mpsc;

pub struct Downloader {
    pub client: HttpClient,
    pub secure_links: SecureLinksManager,
}
impl Downloader {
    pub async fn new(client: HttpClient, secure_links: SecureLinksManager) -> Self {
        Self {
            client,
            secure_links,
        }
    }
    pub async fn verify(
        &self,
        files: Vec<DownloadableFiles>,
        path: &str,
        tx: mpsc::UnboundedSender<VerifyEvent>,
    ) -> Result<(), DownloadError> {
        let resolver = Arc::new(PathResolver::new(PathBuf::from(path)).await?);

        let files = files
            .iter()
            .map(|files| files.product_files.clone())
            .flatten()
            .collect::<Vec<_>>();

        let hash_concurrency = thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        stream::iter(files)
            .map(|file| {
                let resolver = resolver.clone();
                let tx = tx.clone();
                async move {
                    let final_path = match resolver.resolve_path(&file.path).await {
                        Ok(path) => path,
                        Err(e) => {
                            tx.send(VerifyEvent::FileNotFound(file.path.clone())).ok();
                            return Err(DownloadError::from(e));
                        }
                    };

                    let (algo, expected_checksum) = {
                        if let Some(sha256) = &file.sha256 {
                            (ChecksumAlgorithm::Sha256, sha256.clone())
                        } else if let Some(md5) = &file.md5 {
                            (ChecksumAlgorithm::Md5, md5.clone())
                        } else {
                            // Single chunk file, verifying with chunk checksum
                            let chunk_checksum = file
                                .chunks
                                .as_ref()
                                .and_then(|c| c.first())
                                .map(|c| c.md5.clone());
                            if let Some(chunk_checksum) = chunk_checksum {
                                (ChecksumAlgorithm::Md5, chunk_checksum)
                            } else {
                                // No chunk checksum, assuming file is OK
                                tx.send(VerifyEvent::FileOk).ok();
                                return Ok::<(), DownloadError>(());
                            }
                        }
                    };

                    let actual_checksum = match compute_checksum(final_path.clone(), algo).await {
                        Ok(checksum) => checksum,
                        Err(_e) => {
                            tx.send(VerifyEvent::FileChecksumMismatch(
                                final_path
                                    .file_name()
                                    .unwrap_or_default()
                                    .to_string_lossy()
                                    .to_string(),
                            ))
                            .ok();
                            return Ok::<(), DownloadError>(());
                        }
                    };

                    if actual_checksum != expected_checksum {
                        tx.send(VerifyEvent::FileChecksumMismatch(
                            final_path
                                .file_name()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .to_string(),
                        ))
                        .ok();
                        return Ok::<(), DownloadError>(());
                    }

                    tx.send(VerifyEvent::FileOk).ok();
                    Ok::<(), DownloadError>(())
                }
            })
            .buffer_unordered(hash_concurrency)
            .collect::<Vec<_>>()
            .await;
        Ok(())
    }
}

pub enum VerifyEvent {
    FileOk,
    FileChecksumMismatch(String),
    FileNotFound(String),
}
