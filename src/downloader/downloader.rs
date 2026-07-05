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
        println!("Hash concurrency: {}", hash_concurrency);
        stream::iter(files)
            .map(|file| {
                let resolver = resolver.clone();
                let tx = tx.clone();
                async move {
                    let final_path = resolver.resolve_path(&file.path).await?;
                    let (algo, expected_checksum) = {
                        if let Some(sha256) = &file.sha256 {
                            (ChecksumAlgorithm::Sha256, sha256.clone())
                        } else if let Some(md5) = &file.md5 {
                            (ChecksumAlgorithm::Md5, md5.clone())
                        } else {
                            tx.send(VerifyEvent::FileOk).ok();
                            return Ok::<(), DownloadError>(());
                        }
                    };

                    let actual_checksum = match compute_checksum(final_path.clone(), algo).await {
                        Ok(checksum) => checksum,
                        Err(e) => {
                            println!("Error: {}", e);
                            println!("Checksum error for file: {}", file.path);
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
                        println!("Checksum mismatch for file: {}", file.path);
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

                    println!("File: {}: OK", final_path.to_str().unwrap_or("default"));
                    tx.send(VerifyEvent::FileOk).ok();
                    Ok::<(), DownloadError>(())
                }
            })
            .buffer_unordered(hash_concurrency)
            .collect::<Vec<_>>()
            .await;
        println!("Verification completed.");
        Ok(())
    }
}

pub enum VerifyEvent {
    FileOk,
    FileChecksumMismatch(String),
}
