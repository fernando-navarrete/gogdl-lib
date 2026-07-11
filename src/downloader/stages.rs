//! Per-stage filesystem/network work for a download job: allocating files
//! on disk, verifying existing files/chunks, and downloading chunks. Each
//! stage is driven by `Downloader`'s orchestration methods in
//! `downloader.rs`, which build one `PathResolver` per job and share it
//! across every stage call here.

use std::{sync::Arc, thread};

use futures::{StreamExt, stream};
use tokio::{fs, sync::mpsc};

use crate::{
    depot::{DepotFile, DownloadUnit},
    downloader::{
        DownloadError,
        downloader::Downloader,
        events::{DownloadEvent, FileAllocationEvent, FileVerifyEvent, VerifyChunksEvent},
        stream::{UnitAttemptError, stream_unit_to_file},
        util::{ChecksumAlgorithm, PathResolver, compute_chunk_checksum},
    },
};

/// The default fan-out for per-file/per-chunk stage work: the number of
/// logical CPUs, falling back to 4 if that can't be determined.
pub(crate) fn default_concurrency() -> usize {
    thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
}

impl Downloader {
    pub(crate) async fn download_chunk(
        &self,
        units: Vec<DownloadUnit>,
        resolver: Arc<PathResolver>,
        product_id: &str,
        number_of_products: usize,
        tx: mpsc::UnboundedSender<DownloadEvent>,
    ) -> Result<(), DownloadError> {
        let concurrency = (default_concurrency() / number_of_products.max(1)).max(1);

        let secure_links = self.secure_links.get_secure_links(product_id).await?;

        stream::iter(units)
            .map(|unit| {
                let resolver = resolver.clone();
                let secure_links = secure_links.clone();
                let auth = self.auth.clone();
                let client = self.client.clone();
                let tx = tx.clone();
                async move {
                    let url = match secure_links
                        .get_highest_priority_url()
                        .ok_or(DownloadError::BuildNotFound)
                    {
                        Ok(url) => url,
                        Err(_err) => {
                            println!(
                                "[SECURE_LINK_MISSING] {}: no secure link available",
                                unit.path.clone()
                            );
                            tx.send(DownloadEvent::SecureLinkError(unit.path.clone()))
                                .ok();
                            return;
                        }
                    };
                    let primary_url = url.parse_url(&unit.compressed_md5);
                    let redist_url = url.parse_url_redist(&unit.compressed_md5);

                    let access_token = match auth.get_auth().await {
                        Some(auth) => auth.access_token,
                        None => {
                            println!("[NOT_AUTHENTICATED] {}", unit.path.clone());
                            tx.send(DownloadEvent::DownloadError(unit.path.clone()))
                                .ok();
                            return;
                        }
                    };

                    let file_path = match resolver.resolve_path(&unit.path).await {
                        Ok(file_path) => file_path,
                        Err(e) => {
                            println!("[PATH_RESOLVE_ERROR] {}: {}", unit.path.clone(), e);
                            tx.send(DownloadEvent::PathResolveError(unit.path.clone()))
                                .ok();
                            return;
                        }
                    };

                    let mut file = match fs::OpenOptions::new().write(true).open(&file_path).await
                    {
                        Ok(file) => file,
                        Err(e) => {
                            println!("[FILE_OPEN_ERROR] {}: {}", unit.path.clone(), e);
                            tx.send(DownloadEvent::WriteError(unit.path.clone())).ok();
                            return;
                        }
                    };
                    println!(
                        "[PRIMARY_ATTEMPT_START] {}: {}",
                        unit.path.clone(),
                        primary_url
                    );
                    let mut counted: u64 = 0;
                    let primary_result = stream_unit_to_file(
                        &client,
                        &primary_url,
                        access_token.clone(),
                        &mut file,
                        unit.offset,
                        &mut counted,
                        &tx,
                    )
                    .await;

                    let result = match primary_result {
                        Ok(()) => {
                            println!("[PRIMARY_ATTEMPT_OK] {}", unit.path.clone());
                            Ok(())
                        }
                        Err(UnitAttemptError::Write(e)) => {
                            println!(
                                "[PRIMARY_ATTEMPT_WRITE_ERROR] {}: {}",
                                unit.path.clone(),
                                e
                            );
                            Err(UnitAttemptError::Write(e))
                        }
                        Err(UnitAttemptError::Download(err)) => {
                            // The primary secure-link CDN failed for this chunk (e.g. a
                            // redist/dependency chunk it doesn't serve) — retry once
                            // against the redist store, which hosts the same content
                            // under a different path.
                            println!(
                                "[PRIMARY_ATTEMPT_DOWNLOAD_ERROR] {}: {} (falling back to redist store)",
                                unit.path.clone(),
                                err
                            );
                            println!(
                                "[REDIST_ATTEMPT_START] {}: {}",
                                unit.path.clone(),
                                redist_url
                            );
                            let redist_result = stream_unit_to_file(
                                &client,
                                &redist_url,
                                access_token,
                                &mut file,
                                unit.offset,
                                &mut counted,
                                &tx,
                            )
                            .await;
                            match &redist_result {
                                Ok(()) => {
                                    println!("[REDIST_ATTEMPT_OK] {}", unit.path.clone());
                                }
                                Err(UnitAttemptError::Write(e)) => {
                                    println!(
                                        "[REDIST_ATTEMPT_WRITE_ERROR] {}: {}",
                                        unit.path.clone(),
                                        e
                                    );
                                }
                                Err(UnitAttemptError::Download(e)) => {
                                    println!(
                                        "[REDIST_ATTEMPT_DOWNLOAD_ERROR] {}: {}",
                                        unit.path.clone(),
                                        e
                                    );
                                }
                            }
                            redist_result
                        }
                    };

                    match result {
                        Ok(()) => {
                            println!("[CHUNK_DOWNLOADED] {}", unit.path.clone());
                            tx.send(DownloadEvent::ChunkDownloaded {
                                path: unit.path.clone(),
                            })
                            .ok();
                        }
                        Err(UnitAttemptError::Write(e)) => {
                            println!("[FINAL_WRITE_ERROR] {}: {}", unit.path.clone(), e);
                            tx.send(DownloadEvent::WriteError(unit.path.clone())).ok();
                        }
                        Err(UnitAttemptError::Download(err)) => {
                            println!("[FINAL_DOWNLOAD_ERROR] {}: {}", unit.path.clone(), err);
                            tx.send(DownloadEvent::DownloadError(unit.path.clone()))
                                .ok();
                        }
                    }
                }
            })
            .buffer_unordered(concurrency)
            .collect::<Vec<_>>()
            .await;
        Ok(())
    }
    pub(crate) async fn verify_chunks(
        &self,
        resolver: Arc<PathResolver>,
        download_units: Vec<DownloadUnit>,
        tx: mpsc::UnboundedSender<VerifyChunksEvent>,
    ) -> Result<Vec<DownloadUnit>, DownloadError> {
        let concurrency = default_concurrency();

        let results = stream::iter(download_units)
            .map(|unit| {
                let resolver = resolver.clone();
                let tx = tx.clone();
                async move {
                    // At this point, the unit is already resolved, so we can use it directly
                    let path_opt = match resolver.resolve_existing_path(&unit.path).await {
                        Ok(path_opt) => path_opt,
                        Err(e) => {
                            println!("Failed to resolve path for unit {}: {}", unit.path, e);
                            tx.send(VerifyChunksEvent::PathResolveError(unit.path.clone()))
                                .ok();
                            return Some(unit.clone());
                        }
                    };

                    let path = match path_opt {
                        Some(path) => path,
                        None => {
                            println!("File not found for unit {}", unit.path);
                            tx.send(VerifyChunksEvent::FileNotFound(unit.path.clone()))
                                .ok();
                            return Some(unit.clone());
                        }
                    };
                    let algo = ChecksumAlgorithm::Md5;
                    let expected_checksum = unit.md5.clone();

                    let actual_checksum =
                        match compute_chunk_checksum(path.clone(), unit.offset, unit.size, algo)
                            .await
                        {
                            Ok(checksum) => checksum,
                            Err(e) => {
                                tx.send(VerifyChunksEvent::ChecksumCalculationError(
                                    unit.path.clone(),
                                ))
                                .ok();
                                println!("ERROR: {}: {}", unit.path.clone(), e);
                                return Some(unit.clone());
                            }
                        };

                    if actual_checksum != expected_checksum {
                        tx.send(VerifyChunksEvent::ChunkChecksumMismatch(unit.path.clone()))
                            .ok();
                        println!("ERROR: {}: checksum mismatch", unit.path.clone());
                        return Some(unit.clone());
                    }

                    None
                }
            })
            .buffer_unordered(concurrency)
            .collect::<Vec<_>>()
            .await;

        let errored_units = results
            .iter()
            .filter(|result| result.is_some())
            .map(|result| result.as_ref().unwrap().clone())
            .collect::<Vec<_>>();
        Ok(errored_units)
    }
    pub(crate) async fn allocate_files(
        &self,
        files: Vec<DepotFile>,
        resolver: Arc<PathResolver>,
        tx: mpsc::UnboundedSender<FileAllocationEvent>,
    ) -> Result<(), DownloadError> {
        let concurrency = default_concurrency();

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
                    tx.send(FileAllocationEvent::FileOk).ok();
                    None
                }
            })
            .buffer_unordered(concurrency)
            .collect::<Vec<_>>()
            .await;

        let errors = results.into_iter().flatten().collect::<Vec<_>>();
        for file in &errors {
            println!("FAILED TO ALLOCATE: {}", file.path);
        }
        if !errors.is_empty() {
            return Err(DownloadError::DiskAllocationError);
        }
        Ok(())
    }
    pub(crate) async fn verify_files(
        &self,
        files: Vec<DepotFile>,
        resolver: Arc<PathResolver>,
        tx: mpsc::UnboundedSender<FileVerifyEvent>,
    ) -> Result<Vec<DepotFile>, DownloadError> {
        let concurrency = default_concurrency();

        let results = stream::iter(files)
            .map(|file| {
                let resolver = resolver.clone();
                let tx = tx.clone();
                async move {
                    let opt_path = match resolver.resolve_existing_path(&file.path).await {
                        Ok(opt) => opt,
                        Err(err) => {
                            tx.send(FileVerifyEvent::CouldNotResolvePath(file.path.clone()))
                                .ok();
                            println!("ERROR: {}: {}", file.path.clone(), err);
                            return Some(file.clone());
                        }
                    };
                    let final_path = match opt_path {
                        Some(path) => path,
                        None => {
                            tx.send(FileVerifyEvent::FileNotFound(file.path.clone()))
                                .ok();
                            println!("ERROR: {}: FILE NOT FOUND", file.path.clone());
                            return Some(file.clone());
                        }
                    };

                    let size = match resolver.get_file_size(&final_path).await {
                        Ok(size) => size,
                        Err(e) => {
                            tx.send(FileVerifyEvent::CouldNotReadFileSize(file.path.clone()))
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
                    tx.send(FileVerifyEvent::FileOk).ok();
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
}
