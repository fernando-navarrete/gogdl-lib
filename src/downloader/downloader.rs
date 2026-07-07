use std::{collections::HashSet, path::PathBuf, sync::Arc, thread};

use crate::{
    DownloadableFiles,
    client::HttpClient,
    depot::{DepotFile, DownloadUnit},
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
    pub async fn repair_download(
        &self,
        files: Vec<DownloadableFiles>,
        path: &str,
    ) -> Result<(), DownloadError> {
        let all_files = files
            .clone()
            .into_iter()
            .flat_map(|f| f.product_files)
            .collect::<Vec<_>>();

        let (tx, mut rx) = mpsc::unbounded_channel::<FileVerifyEvent>();

        let broken_files_future = self.verify_files(all_files.clone(), path, tx);
        let receive_future = async {
            while let Some(event) = rx.recv().await {
                match event {
                    FileVerifyEvent::FileNotFound(_) => todo!(),
                    FileVerifyEvent::CouldNotResolvePath(_) => todo!(),
                    FileVerifyEvent::CouldNotReadFileSize(_) => todo!(),
                    FileVerifyEvent::SizeMismatch(_, _, _) => todo!(),
                    FileVerifyEvent::ChecksumMismatch(_) => todo!(),
                    FileVerifyEvent::FileOk => todo!(),
                }
            }
        };
        let (verify_result, ()) = tokio::join!(broken_files_future, receive_future);

        let verify_result = verify_result?;

        let (tx, mut rx) = mpsc::unbounded_channel::<FileAllocationEvent>();
        let allocate_files_future = self.allocate_files(verify_result.clone(), path, tx);
        let receive_future = async {
            while let Some(event) = rx.recv().await {
                match event {
                    FileAllocationEvent::PathResolveError(_) => todo!(),
                    FileAllocationEvent::FileAllocated(_) => todo!(),
                    FileAllocationEvent::AllocationError(_) => todo!(),
                    FileAllocationEvent::FileSizeError(_) => todo!(),
                    FileAllocationEvent::FileOk => todo!(),
                }
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

        let (tx, mut rx) = mpsc::unbounded_channel::<VerifyChunksEvent>();
        let verify_chunks_future = self.verify_chunks(path, download_units, tx);

        let receive_future = async {
            while let Some(event) = rx.recv().await {
                match event {
                    VerifyChunksEvent::PathResolveError(_) => todo!(),
                    VerifyChunksEvent::FileNotFound(_) => todo!(),
                    VerifyChunksEvent::ChecksumCalculationError(_) => todo!(),
                    VerifyChunksEvent::ChunkChecksumMismatch(_) => todo!(),
                }
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
                download_futures.push(async move {
                    self.download_chunk(filtered_files, path, &file.product_id, number_of_products)
                        .await
                });
            }
        }

        let results = futures::future::join_all(download_futures).await;
        // todo: actual download step
        todo!()
    }
    async fn download_chunk(
        &self,
        unit: Vec<DownloadUnit>,
        path: &str,
        product_id: &str,
        number_of_products: usize,
    ) -> Result<(), DownloadError> {
        let resolver = Arc::new(PathResolver::new(PathBuf::from(path)).await?);

        let concurrency = thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
            / number_of_products;

        let secure_links = self.secure_links.get_secure_links(product_id).await?;

        stream::iter(unit)
            .map(|unit| {
                let resolver = resolver.clone();
                let secure_links = secure_links.clone();
                let client = self.client.clone();
                async move {}
            })
            .buffer_unordered(concurrency)
            .collect::<Vec<_>>()
            .await;
        todo!()
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

struct DownloadableBundle {
    product_id: String,
    product_files: Vec<DownloadUnit>,
}

enum VerifyChunksEvent {
    PathResolveError(String),
    FileNotFound(String),
    ChecksumCalculationError(String),
    ChunkChecksumMismatch(String),
}

enum FileAllocationEvent {
    PathResolveError(String),
    FileAllocated(String),
    AllocationError(String),
    FileSizeError(String),
    FileOk,
}

enum FileVerifyEvent {
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
