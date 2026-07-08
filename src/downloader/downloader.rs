use std::{collections::HashSet, io::SeekFrom, path::PathBuf, sync::Arc, thread};

use crate::{
    DownloadableFiles,
    auth::AuthManager,
    client::{ClientError, HttpClient},
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

/// The outcome of a single fetch-and-write attempt for one chunk, made by
/// `stream_unit_to_file`. Distinguishes a CDN/stream failure (worth retrying
/// against the redist store) from a local disk failure (not worth retrying,
/// since the same disk is used for any fallback attempt too).
enum UnitAttemptError {
    Download(ClientError),
    Write(std::io::Error),
}

impl std::fmt::Display for UnitAttemptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UnitAttemptError::Download(e) => write!(f, "{e}"),
            UnitAttemptError::Write(e) => write!(f, "{e}"),
        }
    }
}

/// Streams `url`'s (decoded) bytes into `file` starting at `offset`, emitting
/// a `DownloadEvent::Progress` for each byte range not yet credited.
///
/// `counted` is a per-unit high-water-mark of how many decoded bytes have
/// already been credited via `Progress` for this chunk, shared across a
/// primary attempt and a possible redist-fallback retry: if a first attempt
/// streams `w` bytes before failing, the retry only emits `Progress` for
/// bytes beyond `w`, so the two attempts together credit exactly the chunk's
/// size once, never double-counting the re-streamed prefix.
async fn stream_unit_to_file(
    client: &HttpClient,
    url: &str,
    access_token: String,
    file: &mut fs::File,
    offset: u64,
    counted: &mut u64,
    tx: &mpsc::UnboundedSender<DownloadEvent>,
) -> Result<(), UnitAttemptError> {
    file.seek(SeekFrom::Start(offset))
        .await
        .map_err(UnitAttemptError::Write)?;

    let mut rx = client.fetch_chunk_stream(url, access_token);
    let mut pos: u64 = 0;

    loop {
        match rx.recv().await {
            Some(Ok(bytes)) => {
                file.write_all(&bytes).await.map_err(UnitAttemptError::Write)?;
                pos += bytes.len() as u64;
                if pos > *counted {
                    tx.send(DownloadEvent::Progress {
                        bytes: pos - *counted,
                    })
                    .ok();
                    *counted = pos;
                }
            }
            Some(Err(err)) => return Err(UnitAttemptError::Download(err)),
            None => break,
        }
    }
    file.flush().await.ok();
    Ok(())
}

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
    /// Downloads every file in `files` into `path` from scratch: allocates
    /// all files on disk, then downloads every chunk. Unlike
    /// `repair_download`, there is no existing install to diff against, so
    /// no verification stages run — every file is allocated and every chunk
    /// is downloaded unconditionally.
    pub async fn download(
        &self,
        files: Vec<DownloadableFiles>,
        path: &str,
        tx: mpsc::UnboundedSender<DownloadJobEvent>,
    ) -> Result<(), DownloadError> {
        let all_files = files
            .iter()
            .flat_map(|f| f.product_files.clone())
            .collect::<Vec<_>>();

        let (stage_tx, mut stage_rx) = mpsc::unbounded_channel::<FileAllocationEvent>();
        let out_tx = tx.clone();
        let allocate_files_future = self.allocate_files(all_files, path, stage_tx);
        let receive_future = async move {
            while let Some(event) = stage_rx.recv().await {
                out_tx
                    .send(DownloadJobEvent {
                        stage: DownloadStage::Allocating,
                        detail: DownloadDetail::Allocation(event),
                    })
                    .ok();
            }
        };
        let (allocate_result, ()) = tokio::join!(allocate_files_future, receive_future);
        allocate_result?;

        let (stage_tx, mut stage_rx) = mpsc::unbounded_channel::<DownloadEvent>();
        let out_tx = tx.clone();
        let receive_future = async move {
            while let Some(event) = stage_rx.recv().await {
                out_tx
                    .send(DownloadJobEvent {
                        stage: DownloadStage::Downloading,
                        detail: DownloadDetail::Download(event),
                    })
                    .ok();
            }
        };

        let (total_bytes, total_chunks) = files
            .iter()
            .flat_map(|f| f.product_files.iter())
            .flat_map(|df| df.get_download_units())
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
            let units = file
                .product_files
                .iter()
                .flat_map(|depot_file| depot_file.get_download_units())
                .collect::<Vec<_>>();
            if !units.is_empty() {
                let stage_tx = stage_tx.clone();
                download_futures.push(async move {
                    self.download_chunk(units, path, &file.product_id, number_of_products, stage_tx)
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

/// Which stage of `Downloader::download` a `DownloadJobEvent` originated from.
pub enum DownloadStage {
    Allocating,
    Downloading,
}

/// The stage-specific event wrapped by a `DownloadJobEvent`.
pub enum DownloadDetail {
    Allocation(FileAllocationEvent),
    Download(DownloadEvent),
}

/// A single status update from `Downloader::download`, tagged with the
/// stage it came from so callers tracking a multi-stage download can tell
/// them apart on one channel.
pub struct DownloadJobEvent {
    pub stage: DownloadStage,
    pub detail: DownloadDetail,
}
