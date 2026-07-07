use std::{path::PathBuf, sync::Arc, thread};

use crate::{
    DownloadableFiles,
    client::HttpClient,
    downloader::{
        DownloadError,
        util::{ChecksumAlgorithm, PathResolver, compute_chunk_checksum},
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

        let concurrency = thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);

        let files = files
            .iter()
            .map(|files| files.product_files.clone())
            .flatten()
            .collect::<Vec<_>>();

        let chunks = files
            .iter()
            .map(|chunk| chunk.get_download_units())
            .flatten()
            .collect::<Vec<_>>();

        stream::iter(chunks)
            .map(|chunk| {
                let resolver = resolver.clone();
                let tx = tx.clone();
                async move {
                    let final_path = match resolver.resolve_existing_path(&chunk.path).await {
                        Ok(path) => match path {
                            Some(path) => path,
                            None => {
                                tx.send(VerifyEvent::FileNotFound(chunk.path.clone())).ok();
                                println!("ERROR: {}: FILE NOT FOUND", chunk.path.clone());
                                return Some(chunk.path.clone());
                            }
                        },
                        Err(e) => {
                            tx.send(VerifyEvent::FileNotFound(chunk.path.clone())).ok();
                            println!("ERROR: {}: {}", chunk.path.clone(), e);
                            return Some(chunk.path.clone());
                        }
                    };

                    let algo = ChecksumAlgorithm::Md5;
                    let expected_checksum = chunk.md5.clone();

                    let actual_checksum = match compute_chunk_checksum(
                        final_path.clone(),
                        chunk.offset,
                        chunk.size,
                        algo,
                    )
                    .await
                    {
                        Ok(checksum) => checksum,
                        Err(e) => {
                            tx.send(VerifyEvent::ChunkChecksumMismatch(
                                final_path.to_string_lossy().to_string(),
                            ))
                            .ok();
                            println!("ERROR: {}: {}", chunk.path.clone(), e);
                            return Some(chunk.path.clone());
                        }
                    };

                    if actual_checksum != expected_checksum {
                        tx.send(VerifyEvent::ChunkChecksumMismatch(
                            final_path.to_string_lossy().to_string(),
                        ))
                        .ok();
                        println!("ERROR: {} CHECKSUM MISMATCH", chunk.path.clone());
                        return Some(chunk.path.clone());
                    }

                    tx.send(VerifyEvent::ChunkOk).ok();
                    return None;
                }
            })
            .buffer_unordered(concurrency)
            .collect::<Vec<_>>()
            .await;

        Ok(())
    }
}

pub enum VerifyEvent {
    ChunkOk,
    ChunkChecksumMismatch(String),
    FileNotFound(String),
}
pub enum DownloadEvent {}
