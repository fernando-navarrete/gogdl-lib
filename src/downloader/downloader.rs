use std::{collections::HashSet, io::SeekFrom, path::PathBuf, sync::Arc, thread};

use crate::{
    DownloadableFiles,
    auth::AuthManager,
    client::HttpClient,
    depot::{DepotFile, DownloadUnit},
    downloader::{
        DownloadError,
        util::{ChecksumAlgorithm, PathResolver, compute_chunk_checksum},
    },
    secure_links::SecureLinksManager,
};
use futures::{StreamExt, stream};
use tokio::{
    fs,
    io::{AsyncSeekExt, AsyncWriteExt},
    sync::mpsc,
};

pub struct Downloader {
    pub client: HttpClient,
    pub secure_links: SecureLinksManager,
    pub auth: AuthManager,
}
impl Downloader {
    pub async fn new(
        client: HttpClient,
        secure_links: SecureLinksManager,
        auth: AuthManager,
    ) -> Self {
        Self {
            client,
            secure_links,
            auth,
        }
    }
    pub async fn repair_download(
        &self,
        files: Vec<DownloadableFiles>,
        path: &str,
        tx: mpsc::UnboundedSender<RepairEvent>,
    ) -> Result<(), DownloadError> {
        let all_files = files
            .clone()
            .into_iter()
            .flat_map(|f| f.product_files)
            .collect::<Vec<_>>();

        let (stage_tx, mut stage_rx) = mpsc::unbounded_channel::<FileVerifyEvent>();
        let out_tx = tx.clone();
        let broken_files_future = self.verify_files(all_files.clone(), path, stage_tx);
        let receive_future = async move {
            while let Some(event) = stage_rx.recv().await {
                out_tx
                    .send(RepairEvent {
                        stage: RepairStage::VerifyingFiles,
                        detail: RepairDetail::FileVerify(event),
                    })
                    .ok();
            }
        };
        let (verify_result, ()) = tokio::join!(broken_files_future, receive_future);

        let verify_result = verify_result?;

        let (stage_tx, mut stage_rx) = mpsc::unbounded_channel::<FileAllocationEvent>();
        let out_tx = tx.clone();
        let allocate_files_future = self.allocate_files(verify_result.clone(), path, stage_tx);
        let receive_future = async move {
            while let Some(event) = stage_rx.recv().await {
                out_tx
                    .send(RepairEvent {
                        stage: RepairStage::Allocating,
                        detail: RepairDetail::Allocation(event),
                    })
                    .ok();
            }
        };
        let (allocate_result, ()) = tokio::join!(allocate_files_future, receive_future);
        let _allocate_result = allocate_result?;

        let needs_repair_paths: HashSet<&str> =
            verify_result.iter().map(|f| f.path.as_str()).collect();

        let unaffected_files = all_files
            .iter()
            .filter(|f| !needs_repair_paths.contains(&f.path.as_str()))
            .cloned()
            .collect::<Vec<_>>();

        let download_units = unaffected_files
            .iter()
            .flat_map(|depot_file| depot_file.get_download_units())
            .collect::<Vec<_>>();

        let (stage_tx, mut stage_rx) = mpsc::unbounded_channel::<VerifyChunksEvent>();
        let out_tx = tx.clone();
        let verify_chunks_future = self.verify_chunks(path, download_units, stage_tx);

        let receive_future = async move {
            while let Some(event) = stage_rx.recv().await {
                out_tx
                    .send(RepairEvent {
                        stage: RepairStage::VerifyingChunks,
                        detail: RepairDetail::ChunkVerify(event),
                    })
                    .ok();
            }
        };
        let (errored_units_result, ()) = tokio::join!(verify_chunks_future, receive_future);
        let mut errored_units = errored_units_result?; // <- This is the final file of chunks to download

        let missing_from_first_step = verify_result
            .iter()
            .flat_map(|depot_file| depot_file.get_download_units())
            .collect::<Vec<_>>();

        errored_units.extend(missing_from_first_step);

        let needs_download_map: HashSet<&str> =
            errored_units.iter().map(|f| f.path.as_str()).collect();

        let (stage_tx, mut stage_rx) = mpsc::unbounded_channel::<DownloadEvent>();
        let out_tx = tx.clone();
        let receive_future = async move {
            while let Some(event) = stage_rx.recv().await {
                out_tx
                    .send(RepairEvent {
                        stage: RepairStage::Downloading,
                        detail: RepairDetail::Download(event),
                    })
                    .ok();
            }
        };

        let (total_bytes, total_chunks) = files
            .iter()
            .flat_map(|f| f.product_files.iter())
            .flat_map(|df| df.get_download_units())
            .filter(|u| needs_download_map.contains(&u.path.as_str()))
            .fold((0u64, 0usize), |(bytes, chunks), u| (bytes + u.size, chunks + 1));
        stage_tx
            .send(DownloadEvent::Total {
                bytes: total_bytes,
                chunks: total_chunks,
            })
            .ok();

        let mut download_futures = Vec::new();
        let number_of_products = files.len();
        for file in files {
            let filtered_files = file
                .product_files
                .iter()
                .flat_map(|depot_file| depot_file.get_download_units())
                .filter(|unit| needs_download_map.contains(&unit.path.as_str()))
                .collect::<Vec<_>>();
            if !filtered_files.is_empty() {
                let stage_tx = stage_tx.clone();
                download_futures.push(async move {
                    self.download_chunk(
                        filtered_files,
                        path,
                        &file.product_id,
                        number_of_products,
                        stage_tx,
                    )
                    .await
                });
            }
        }
        // Drop our own handle so the channel closes (and `receive_future`
        // returns) once every spawned download future has finished and
        // dropped its cloned sender.
        drop(stage_tx);

        let (results, ()) =
            tokio::join!(futures::future::join_all(download_futures), receive_future);

        for result in results {
            result?;
        }

        Ok(())
    }
    async fn download_chunk(
        &self,
        units: Vec<DownloadUnit>,
        path: &str,
        product_id: &str,
        number_of_products: usize,
        tx: mpsc::UnboundedSender<DownloadEvent>,
    ) -> Result<(), DownloadError> {
        let resolver = Arc::new(PathResolver::new(PathBuf::from(path)).await?);

        let concurrency = (thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
            / number_of_products.max(1))
        .max(1);

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
                            println!("ERROR: {}: no secure link available", unit.path.clone());
                            tx.send(DownloadEvent::SecureLinkError(unit.path.clone()))
                                .ok();
                            return;
                        }
                    };
                    let url = url.parse_url(&unit.compressed_md5);

                    let access_token = match auth.get_auth().await {
                        Some(auth) => auth.access_token,
                        None => {
                            println!("ERROR: {}: not authenticated", unit.path.clone());
                            tx.send(DownloadEvent::DownloadError(unit.path.clone()))
                                .ok();
                            return;
                        }
                    };

                    let file_path = match resolver.resolve_path(&unit.path).await {
                        Ok(file_path) => file_path,
                        Err(e) => {
                            println!("ERROR: {}: {}", unit.path.clone(), e);
                            tx.send(DownloadEvent::PathResolveError(unit.path.clone()))
                                .ok();
                            return;
                        }
                    };

                    let mut file = match fs::OpenOptions::new().write(true).open(&file_path).await
                    {
                        Ok(file) => file,
                        Err(e) => {
                            println!("ERROR: {}: {}", unit.path.clone(), e);
                            tx.send(DownloadEvent::WriteError(unit.path.clone())).ok();
                            return;
                        }
                    };
                    if let Err(e) = file.seek(SeekFrom::Start(unit.offset)).await {
                        println!("ERROR: {}: {}", unit.path.clone(), e);
                        tx.send(DownloadEvent::WriteError(unit.path.clone())).ok();
                        return;
                    }

                    let mut rx = client.fetch_chunk_stream(&url, access_token);

                    loop {
                        match rx.recv().await {
                            Some(Ok(bytes)) => {
                                if let Err(e) = file.write_all(&bytes).await {
                                    println!("ERROR: {}: {}", unit.path.clone(), e);
                                    tx.send(DownloadEvent::WriteError(unit.path.clone())).ok();
                                    return;
                                }
                                tx.send(DownloadEvent::Progress {
                                    bytes: bytes.len() as u64,
                                })
                                .ok();
                            }
                            Some(Err(err)) => {
                                println!("ERROR: {}: {}", unit.path.clone(), err);
                                tx.send(DownloadEvent::DownloadError(unit.path.clone()))
                                    .ok();
                                return;
                            }
                            None => break,
                        }
                    }
                    file.flush().await.ok();

                    tx.send(DownloadEvent::ChunkDownloaded {
                        path: unit.path.clone(),
                    })
                    .ok();
                }
            })
            .buffer_unordered(concurrency)
            .collect::<Vec<_>>()
            .await;
        Ok(())
    }
    async fn verify_chunks(
        &self,
        path: &str,
        download_units: Vec<DownloadUnit>,
        tx: mpsc::UnboundedSender<VerifyChunksEvent>,
    ) -> Result<Vec<DownloadUnit>, DownloadError> {
        let resolver = Arc::new(PathResolver::new(PathBuf::from(path)).await?);

        let concurrency = thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);

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
    async fn allocate_files(
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
    async fn verify_files(
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
                    let opt_path = match resolver.resolve_existing_path(&chunk.path).await {
                        Ok(path) => path,
                        Err(e) => {
                            tx.send(VerifyEvent::CouldNotResolvePath(chunk.path.clone()))
                                .ok();
                            println!("ERROR: {}: {}", chunk.path.clone(), e);
                            return Some(chunk.path.clone());
                        }
                    };

                    let final_path = match opt_path {
                        Some(path) => path,
                        None => {
                            tx.send(VerifyEvent::FileNotFound(chunk.path.clone())).ok();
                            println!("ERROR: {}: FILE NOT FOUND", chunk.path.clone());
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

pub enum VerifyChunksEvent {
    PathResolveError(String),
    FileNotFound(String),
    ChecksumCalculationError(String),
    ChunkChecksumMismatch(String),
}

pub enum FileAllocationEvent {
    PathResolveError(String),
    FileAllocated(String),
    AllocationError(String),
    FileSizeError(String),
    FileOk,
}

pub enum FileVerifyEvent {
    FileNotFound(String),
    CouldNotResolvePath(String),
    CouldNotReadFileSize(String),
    SizeMismatch(String, u64, u64),
    ChecksumMismatch(String),
    FileOk,
}

pub enum VerifyEvent {
    CouldNotResolvePath(String),
    FileNotFound(String),
    ChunkChecksumMismatch(String),
    ChunkOk,
}

/// Emitted by the download stage of `repair_download`.
pub enum DownloadEvent {
    /// Sent once at the start of the download stage: the total decoded
    /// bytes and chunk count queued for download. Clients can track
    /// progress as `downloaded = Σ Progress.bytes` and
    /// `remaining = Total.bytes - downloaded`.
    Total { bytes: u64, chunks: usize },
    /// An incremental delta of decoded bytes written for a chunk still in
    /// flight. Sent as each network read is decoded and written to disk.
    Progress { bytes: u64 },
    /// A chunk was downloaded and written successfully in full.
    ChunkDownloaded { path: String },
    /// No usable secure link was available for this chunk's product.
    SecureLinkError(String),
    /// The chunk failed to download (network error, stream closed, or not authenticated).
    DownloadError(String),
    /// The chunk downloaded but couldn't be written to disk at its offset.
    WriteError(String),
    /// The destination path for the chunk couldn't be resolved.
    PathResolveError(String),
}

/// Which stage of `repair_download` a `RepairEvent` originated from.
pub enum RepairStage {
    VerifyingFiles,
    Allocating,
    VerifyingChunks,
    Downloading,
}

/// The stage-specific event wrapped by a `RepairEvent`.
pub enum RepairDetail {
    FileVerify(FileVerifyEvent),
    Allocation(FileAllocationEvent),
    ChunkVerify(VerifyChunksEvent),
    Download(DownloadEvent),
}

/// A single status update from `repair_download`, tagged with the stage it
/// came from so callers tracking a multi-stage repair can tell them apart
/// on one channel.
pub struct RepairEvent {
    pub stage: RepairStage,
    pub detail: RepairDetail,
}
