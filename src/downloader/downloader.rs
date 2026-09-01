use std::{path::PathBuf, sync::Arc};

use async_compression::tokio::write::ZlibDecoder;
use futures_util::{StreamExt, stream};
use tokio::io::{AsyncWriteExt, BufWriter};
use tokio::sync::mpsc;

use crate::{
    client::HttpClient,
    depot::DepotFile,
    downloader::{
        DownloadError, DownloadEvent, DownloadUnit, PathResolver, ProductBundle,
        progress_reporting::{
            DownloadStageEvent, FileAllocationEvent, FileSizeVerificationEvent, VerificationEvent,
        },
        util::{ChecksumAlgorithm, HashingWriter, OffsetWriter, compute_chunk_checksum},
    },
    secure_links::SecureLinksManager,
};

pub struct Downloader {
    pub client: HttpClient,
    pub secure_links: SecureLinksManager,
    threads: usize,
}

impl Downloader {
    pub fn new(client: HttpClient, secure_links: SecureLinksManager) -> Self {
        let threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
            .clamp(1, 12);
        Self {
            client,
            secure_links,
            threads,
        }
    }
    pub async fn download(
        &self,
        bundles: Vec<ProductBundle>,
        path: &str,
        tx: mpsc::UnboundedSender<DownloadStageEvent>,
    ) -> Result<(), DownloadError> {
        let path_resolver = Arc::new(PathResolver::new(PathBuf::from(path)).await?);

        let depot_files = bundles
            .iter()
            .flat_map(|bundle| bundle.product_files.clone())
            .collect::<Vec<DepotFile>>();

        // File size verification step
        let (missing_files_tx, mut missing_files_rx) = mpsc::unbounded_channel();
        let missing_files_fut =
            self.verify_files_size(&depot_files, &path_resolver, missing_files_tx);
        let tx_stage1 = tx.clone();
        let progress_future = async move {
            while let Some(event) = missing_files_rx.recv().await {
                tx_stage1
                    .send(DownloadStageEvent::FileSizeVerificationStage(event))
                    .ok();
            }
        };
        let (missing_files, _) = tokio::join!(missing_files_fut, progress_future);

        // File allocation step
        let (files_allocation_tx, mut files_allocation_rx) = mpsc::unbounded_channel();
        let files_allocation_fut =
            self.allocate_missing_files(&missing_files, &path_resolver, files_allocation_tx);
        let tx_stage2 = tx.clone();
        let progress_future = async move {
            while let Some(event) = files_allocation_rx.recv().await {
                tx_stage2
                    .send(DownloadStageEvent::FileAllocationStage(event))
                    .ok();
            }
        };
        let (files_allocation_error, _) = tokio::join!(files_allocation_fut, progress_future);
        drop(missing_files);

        // Check if all files were allocated successfully, if not, we may have run out of disk space
        if files_allocation_error.len() != 0 {
            tx.send(DownloadStageEvent::FileAllocationError()).ok();
            return Err(DownloadError::FileAllocationError);
        }
        drop(files_allocation_error);

        // Download step

        let (files_download_tx, mut files_download_rx) = mpsc::unbounded_channel();
        let downloader_future = self.download_files(&bundles, &path_resolver, files_download_tx);
        let tx_stage3 = tx.clone();
        let progress_future = async move {
            while let Some(event) = files_download_rx.recv().await {
                tx_stage3
                    .send(DownloadStageEvent::DownloadStage(event))
                    .ok();
            }
        };
        let (res, _) = tokio::join!(downloader_future, progress_future);
        res?;
        Ok(())
    }
    pub async fn verify(
        &self,
        bundles: Vec<ProductBundle>,
        path: &str,
        tx: mpsc::UnboundedSender<VerificationEvent>,
    ) -> Result<(), DownloadError> {
        let path_resolver = Arc::new(PathResolver::new(PathBuf::from(path)).await?);

        let download_units = bundles
            .iter()
            .flat_map(|bundle| bundle.product_files.clone())
            .flat_map(|depot_file| DownloadUnit::from_depot_file(depot_file))
            .collect::<Vec<DownloadUnit>>();

        let missing_units = self
            .verify_download_units(&download_units, &path_resolver, tx)
            .await;

        let missing_units_count = missing_units.len();

        if missing_units_count > 0 {
            return Err(DownloadError::ChunkIntegrityCheckFailed(
                missing_units_count,
            ));
        }

        Ok(())
    }

    async fn download_files(
        &self,
        bundles: &[ProductBundle],
        path_resolver: &PathResolver,
        tx: mpsc::UnboundedSender<DownloadEvent>,
    ) -> Result<(), DownloadError> {
        tx.send(DownloadEvent::Preparing).ok();
        let download_units: Vec<(String, DownloadUnit)> = bundles
            .iter()
            .flat_map(|bundle| {
                bundle.product_files.iter().flat_map(move |depot_file| {
                    DownloadUnit::from_depot_file(depot_file.clone())
                        .into_iter()
                        .map(|download_unit| (bundle.product_id.clone(), download_unit))
                })
            })
            .collect::<Vec<(String, DownloadUnit)>>();

        // Pre-fetch secure links for all bundles so they get stored in cache
        stream::iter(bundles)
            .map(|bundle| {
                let product_id = bundle.product_id.clone();
                async move {
                    let _ = self.secure_links.get_secure_links(&product_id).await;
                }
            })
            .buffer_unordered(self.threads)
            .collect::<Vec<()>>()
            .await;

        tx.send(DownloadEvent::Prepared).ok();
        let results: Vec<Result<(), DownloadError>> = stream::iter(download_units)
            .map(|(product_id, download_unit)| {
                let path_resolver = path_resolver;
                tx.send(DownloadEvent::Downloading).ok();
                let tx = tx.clone();
                async move {
                    let secure_links_manager = &self.secure_links.clone();

                    let file = path_resolver.open_file(&download_unit.path, true).await?;

                    let offset_writer =
                        OffsetWriter::new(file, download_unit.offset, download_unit.size)
                            .await
                            .map_err(DownloadError::DeflateError)?;
                    let sink =
                        HashingWriter::new(BufWriter::with_capacity(1024 * 1024, offset_writer));
                    let mut decoder = ZlibDecoder::new(sink);

                    self.client
                        .stream_chunk(secure_links_manager, &product_id, download_unit.file_type, &download_unit.compressed_md5, async |chunk| {
                            tx.send(DownloadEvent::Progress(chunk.len())).ok();
                            decoder.write_all(&chunk).await
                        })
                        .await?;

                    decoder
                        .shutdown()
                        .await
                        .map_err(DownloadError::DeflateError)?;
                    let (buf_writer, actual_md5) = decoder.into_inner().into_parts();
                    let writer = buf_writer.into_inner();

                    if writer.remaining() != 0 || actual_md5 != download_unit.md5 {
                        return Err(DownloadError::DeflateError(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            format!(
                                "chunk verification failed for '{}' at offset {} (short by {} byte(s), checksum {})",
                                download_unit.path,
                                download_unit.offset,
                                writer.remaining(),
                                if actual_md5 == download_unit.md5 { "ok" } else { "mismatch" },
                            ),
                        )));
                    }

                    Ok(())
                }
            })
            .buffer_unordered(self.threads)
            .collect::<Vec<_>>()
            .await;

        results.into_iter().collect::<Result<(), DownloadError>>()?;
        Ok(())
    }

    async fn allocate_missing_files(
        &self,
        files: &[DepotFile],
        path_resolver: &PathResolver,
        tx: mpsc::UnboundedSender<FileAllocationEvent>,
    ) -> Vec<DepotFile> {
        let failed_files = stream::iter(files)
            .map(|file| {
                let path_resolver = path_resolver;
                let tx = tx.clone();
                async move {
                    let expected_file_size = match file.chunks.as_ref() {
                        Some(chunks) => chunks.iter().fold(0, |acc, chunk| acc + chunk.size),
                        None => {
                            tx.send(FileAllocationEvent::FileWithNoChunks(file.path.clone(), 0))
                                .ok();
                            return None;
                        }
                    };
                    let _opt_path = match path_resolver.resolve_existing_path(&file.path).await {
                        Ok(path) => path,
                        Err(_) => {
                            tx.send(FileAllocationEvent::CouldNotResolvePath(
                                file.path.clone(),
                                expected_file_size,
                            ))
                            .ok();
                            return Some(file);
                        }
                    };

                    match path_resolver
                        .allocate_file(&file.path, expected_file_size)
                        .await
                    {
                        Ok(_) => {
                            tx.send(FileAllocationEvent::FileAllocationSuccess(
                                file.path.clone(),
                                expected_file_size,
                            ))
                            .ok();
                            None
                        }
                        Err(_e) => Some(file),
                    }
                }
            })
            .buffer_unordered(self.threads)
            .collect::<Vec<_>>()
            .await;

        let failed_files = failed_files
            .iter()
            .filter(|&unit| unit.is_some())
            .map(|unit| unit.unwrap().clone())
            .collect::<Vec<_>>();
        failed_files
    }

    async fn verify_files_size(
        &self,
        files: &[DepotFile],
        path_resolver: &PathResolver,
        tx: mpsc::UnboundedSender<FileSizeVerificationEvent>,
    ) -> Vec<DepotFile> {
        let missing_files = stream::iter(files)
            .map(|file| {
                let path_resolver = path_resolver;
                let tx = tx.clone();
                async move {
                    let expected_file_size = match file.chunks.as_ref() {
                        Some(chunks) => chunks.iter().fold(0, |acc, chunk| acc + chunk.size),
                        None => {
                            tx.send(FileSizeVerificationEvent::FileWithNoChunks(
                                file.path.clone(),
                                0,
                            ))
                            .ok();
                            return None;
                        }
                    };
                    let opt_path = match path_resolver.resolve_existing_path(&file.path).await {
                        Ok(path) => path,
                        Err(_) => {
                            tx.send(FileSizeVerificationEvent::CouldNotResolvePath(
                                file.path.clone(),
                                expected_file_size,
                            ))
                            .ok();
                            return Some(file);
                        }
                    };

                    let final_path = match opt_path {
                        Some(path) => path,
                        None => {
                            tx.send(FileSizeVerificationEvent::FileNotFound(
                                file.path.clone(),
                                expected_file_size,
                            ))
                            .ok();
                            return Some(file);
                        }
                    };

                    // File exists, verify size
                    let file_size = match path_resolver.get_file_size(&final_path).await {
                        Ok(size) => size,
                        Err(_) => {
                            tx.send(FileSizeVerificationEvent::FileSizeVerificationFailed(
                                file.path.clone(),
                                expected_file_size,
                            ))
                            .ok();
                            return Some(file);
                        }
                    };

                    if file_size != expected_file_size {
                        tx.send(FileSizeVerificationEvent::FileSizeMismatch(
                            file.path.clone(),
                            expected_file_size,
                        ))
                        .ok();
                        return Some(file);
                    }

                    tx.send(FileSizeVerificationEvent::FileSizeVerificationSuccess(
                        file.path.clone(),
                        expected_file_size,
                    ))
                    .ok();
                    None
                }
            })
            .buffer_unordered(self.threads)
            .collect::<Vec<_>>()
            .await;

        let missing_units = missing_files
            .iter()
            .filter(|&unit| unit.is_some())
            .map(|unit| unit.unwrap().clone())
            .collect::<Vec<_>>();
        missing_units
    }

    async fn verify_download_units(
        &self,
        download_units: &[DownloadUnit],
        path_resolver: &PathResolver,
        tx: mpsc::UnboundedSender<VerificationEvent>,
    ) -> Vec<DownloadUnit> {
        let units = stream::iter(download_units)
            .map(|download_unit| {
                let path_resolver = path_resolver;
                let tx = tx.clone();
                async move {
                    let opt_path = match path_resolver
                        .resolve_existing_path(&download_unit.path)
                        .await
                    {
                        Ok(path) => path,
                        Err(_) => {
                            tx.send(VerificationEvent::CouldNotResolvePath(
                                download_unit.path.clone(),
                                download_unit.size,
                            ))
                            .ok();
                            return Some(download_unit);
                        }
                    };

                    let final_path = match opt_path {
                        Some(path) => path,
                        None => {
                            tx.send(VerificationEvent::FileNotFound(
                                download_unit.path.clone(),
                                download_unit.size,
                            ))
                            .ok();
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
                        Err(_) => {
                            tx.send(VerificationEvent::ChecksumMismatch(
                                download_unit.path.clone(),
                                download_unit.size,
                            ))
                            .ok();
                            return Some(download_unit);
                        }
                    };

                    if actual_checksum != download_unit.md5 {
                        tx.send(VerificationEvent::ChecksumMismatch(
                            download_unit.path.clone(),
                            download_unit.size,
                        ))
                        .ok();
                        return Some(download_unit);
                    }

                    tx.send(VerificationEvent::Verified(
                        download_unit.path.clone(),
                        download_unit.size,
                    ))
                    .ok();
                    return None;
                }
            })
            .buffer_unordered(self.threads)
            .collect::<Vec<_>>()
            .await;

        let missing_units = units
            .iter()
            .filter(|&unit| unit.is_some())
            .map(|unit| unit.unwrap().clone())
            .collect::<Vec<_>>();
        missing_units
    }
}
