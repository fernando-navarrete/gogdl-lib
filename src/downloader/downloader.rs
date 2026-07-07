use std::{path::PathBuf, sync::Arc, thread};

use crate::{
    DownloadableFiles,
    client::HttpClient,
    depot::DepotFile,
    downloader::{
        DownloadError,
        util::{ChecksumAlgorithm, PathResolver, compute_chunk_checksum},
    },
    secure_links::SecureLinksManager,
};
use futures::{StreamExt, stream};
use tokio::{fs, sync::mpsc};

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
    pub async fn allocate_files(
        &self,
        files: Vec<DepotFile>,
        path: &str,
        tx: mpsc::UnboundedSender<FileAllocationEvent>,
    ) -> Result<(), DownloadError> {
        let resolver = Arc::new(PathResolver::new(PathBuf::from(path)).await?);

        let concurrency = thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);

        let results = stream::iter(files)
            .map(|file| {
                let resolver = resolver.clone();
                let tx = tx.clone();
                async move {
                    let path = resolver.resolve_existing_path(&file.path).await;
                    let option = match path {
                        Ok(opt) => opt,
                        Err(err) => {
                            tx.send(FileAllocationEvent::PathResolveError(file.path.clone()))
                                .ok();
                            println!("ERROR: {}: {}", file.path.clone(), err);
                            return Some(file.clone());
                        }
                    };

                    let path = match option {
                        Some(path) => path,
                        None => {
                            // File does not exist, allocating
                            if let Err(e) = resolver
                                .allocate_file(&file.path, file.get_file_size())
                                .await
                            {
                                tx.send(FileAllocationEvent::AllocationError(e.to_string()))
                                    .ok();
                                return Some(file.clone());
                            }
                            tx.send(FileAllocationEvent::FileAllocated(file.path.clone()))
                                .ok();
                            return None;
                        }
                    };

                    let size = match resolver.get_file_size(path.as_path()).await {
                        Ok(size) => size,
                        Err(e) => {
                            tx.send(FileAllocationEvent::FileSizeError(e.to_string()))
                                .ok();
                            return Some(file.clone());
                        }
                    };

                    if size != file.get_file_size() {
                        fs::remove_file(&path).await.ok();

                        if let Err(e) = resolver
                            .allocate_file(&file.path, file.get_file_size())
                            .await
                        {
                            tx.send(FileAllocationEvent::AllocationError(e.to_string()))
                                .ok();
                            return Some(file.clone());
                        }
                        tx.send(FileAllocationEvent::FileAllocated(file.path.clone()))
                            .ok();
                        return None;
                    }
                    None
                }
            })
            .buffer_unordered(concurrency)
            .collect::<Vec<_>>()
            .await;

        let errors = results.into_iter().flatten().collect::<Vec<_>>();
        if !errors.is_empty() {
            return Err(DownloadError::DiskAllocationError);
        }
        Ok(())
    }
    pub async fn verify_files(
        &self,
        files: Vec<DepotFile>,
        path: &str,
        tx: mpsc::UnboundedSender<FileVerifyEvent>,
    ) -> Result<Vec<DepotFile>, DownloadError> {
        let resolver = Arc::new(PathResolver::new(PathBuf::from(path)).await?);

        let concurrency = thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);

        let results = stream::iter(files)
            .map(|file| {
                let resolver = resolver.clone();
                let tx = tx.clone();
                async move {
                    let final_path = match resolver.resolve_existing_path(&file.path).await {
                        Ok(path) => match path {
                            Some(path) => path,
                            None => {
                                tx.send(FileVerifyEvent::FileNotFound(file.path.clone()))
                                    .ok();
                                println!("ERROR: {}: FILE NOT FOUND", file.path.clone());
                                return Some(file.clone());
                            }
                        },
                        Err(e) => {
                            tx.send(FileVerifyEvent::FileNotFound(file.path.clone()))
                                .ok();
                            println!("ERROR: {}: {}", file.path.clone(), e);
                            return Some(file.clone());
                        }
                    };

                    let size = match resolver.get_file_size(&final_path).await {
                        Ok(size) => size,
                        Err(e) => {
                            tx.send(FileVerifyEvent::FileNotFound(file.path.clone()))
                                .ok();
                            println!("ERROR: {}: {}", file.path.clone(), e);
                            return Some(file.clone());
                        }
                    };

                    if size != file.get_file_size() {
                        tx.send(FileVerifyEvent::SizeMismatch(
                            file.path.clone(),
                            size,
                            file.get_file_size(),
                        ))
                        .ok();
                        println!(
                            "ERROR: {}: size mismatch: {} != {}",
                            file.path.clone(),
                            size,
                            file.get_file_size()
                        );
                        return Some(file.clone());
                    }
                    None
                }
            })
            .buffer_unordered(concurrency)
            .collect::<Vec<_>>()
            .await;

        let missing_files = results
            .iter()
            .filter(|res| res.as_ref().is_some())
            // unwrap is safe here because we filtered for Some above
            .map(|res| res.as_ref().unwrap().clone())
            .collect::<Vec<_>>();
        Ok(missing_files)
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

pub enum FileAllocationEvent {
    PathResolveError(String),
    FileAllocated(String),
    AllocationError(String),
    FileSizeError(String),
}

pub enum FileVerifyEvent {
    FileNotFound(String),
    SizeMismatch(String, u64, u64),
    ChecksumMismatch(String),
}

pub enum VerifyEvent {
    ChunkOk,
    ChunkChecksumMismatch(String),
    FileNotFound(String),
}
