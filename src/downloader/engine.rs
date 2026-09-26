use std::{collections::HashMap, path::PathBuf, sync::Arc};

use async_compression::tokio::write::ZlibDecoder;
use bytes::Bytes;
use futures_util::{StreamExt, TryStreamExt, stream};
use tokio::io::{AsyncWriteExt, BufWriter};
use tokio::sync::{Mutex, mpsc};

use crate::ClientError;
use crate::client::retry::{MAX_ATTEMPTS, MAX_HASH_ATTEMPTS, retry_now_or_return, retry_or_return};
use crate::downloader::FileType;
use crate::secure_links::SecureLinksError;
use crate::{
    client::HttpClient,
    depot::DepotFile,
    downloader::{
        DownloadError, DownloadEvent, DownloadUnit, ProductBundle,
        progress_reporting::{
            DownloadStageEvent, FileAllocationEvent, FileSizeVerificationEvent, VerificationEvent,
        },
        util::{HashingWriter, OffsetWriter, ProgressGuard, compute_chunk_checksum},
    },
    fs::PathResolver,
    secure_links::SecureLinksManager,
};

pub struct Downloader {
    pub client: HttpClient,
    pub secure_links: SecureLinksManager,
    threads: usize,
    /// Test-only: how many chunk checksums `verify_download_units` computed.
    #[cfg(test)]
    checksums: Arc<std::sync::atomic::AtomicUsize>,
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
            #[cfg(test)]
            checksums: Arc::default(),
        }
    }
    pub async fn repair(
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
            self.verify_files_size(depot_files, path_resolver.clone(), missing_files_tx);
        let tx_stage1 = tx.clone();
        let progress_future = async move {
            while let Some(event) = missing_files_rx.recv().await {
                tx_stage1
                    .send(DownloadStageEvent::FileSizeVerificationStage(event))
                    .ok();
            }
        };
        let (failed_files, _) = tokio::join!(missing_files_fut, progress_future);

        // Where each failed file's old bytes end. `set_len` in stage 2 keeps
        // them and zero-fills everything past, so only that tail is known to
        // need no checksum.
        let old_lengths: HashMap<String, u64> = failed_files
            .iter()
            .filter_map(|(file, old_len)| Some((file.path.clone(), (*old_len)?)))
            .collect();
        let missing_files: Vec<DepotFile> =
            failed_files.into_iter().map(|(file, _)| file).collect();

        // Check if there is space available on disk
        let required_space = missing_files
            .iter()
            .map(|depot_file| depot_file.size().unwrap_or(0))
            .sum::<u64>();

        path_resolver.check_free_space(required_space)?;

        // File allocation step
        let (files_allocation_tx, mut files_allocation_rx) = mpsc::unbounded_channel();
        let files_allocation_fut =
            self.allocate_missing_files(missing_files, path_resolver.clone(), files_allocation_tx);
        let tx_stage2 = tx.clone();
        let progress_future = async move {
            while let Some(event) = files_allocation_rx.recv().await {
                tx_stage2
                    .send(DownloadStageEvent::FileAllocationStage(event))
                    .ok();
            }
        };
        let (files_allocation_error, _) = tokio::join!(files_allocation_fut, progress_future);

        // Check if all files were allocated successfully, if not, we may have run out of disk space
        if !files_allocation_error.is_empty() {
            tx.send(DownloadStageEvent::FileAllocationError()).ok();
            return Err(DownloadError::FileAllocationError);
        }
        drop(files_allocation_error);

        // Units in a range stage 2 just allocated hold zeros and can't match,
        // so they go straight to the download list without an MD5. They still
        // get the `ChecksumMismatch` that hashing the zeros used to produce,
        // so a consumer counting verification events sees the same number.
        // (A chunk whose real content is all zeros used to come out `Verified`
        // here, and is now re-fetched.)
        let (fresh_units, download_units): (Vec<_>, Vec<_>) =
            DownloadUnit::from_product_bundles(bundles)
                .into_iter()
                .partition(|unit| {
                    old_lengths
                        .get(&unit.path)
                        .is_some_and(|&old_len| unit.offset >= old_len)
                });
        let (verification_tx, mut verification_rx) = mpsc::unbounded_channel();
        for unit in &fresh_units {
            verification_tx
                .send(VerificationEvent::ChecksumMismatch(
                    unit.path.clone(),
                    unit.size,
                ))
                .ok();
        }
        let missing_units_fut =
            self.verify_download_units(download_units, path_resolver.clone(), verification_tx);
        let tx_stage3 = tx.clone();
        let progress_future = async move {
            while let Some(event) = verification_rx.recv().await {
                tx_stage3
                    .send(DownloadStageEvent::VerificationStage(event))
                    .ok();
            }
        };
        let (unverified_units, _) = tokio::join!(missing_units_fut, progress_future);
        let missing_units = [fresh_units, unverified_units].concat();

        if missing_units.is_empty() {
            // All units verified, no missing chunks
            return Ok(());
        }

        // Download step
        let (files_download_tx, mut files_download_rx) = mpsc::unbounded_channel();
        let downloader_future =
            self.download_files(missing_units, &path_resolver, files_download_tx);
        let tx_stage4 = tx.clone();
        let progress_future = async move {
            while let Some(event) = files_download_rx.recv().await {
                tx_stage4
                    .send(DownloadStageEvent::DownloadStage(event))
                    .ok();
            }
        };
        let (res, _) = tokio::join!(downloader_future, progress_future);
        res?;
        Ok(())
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
            self.verify_files_size(depot_files, path_resolver.clone(), missing_files_tx);
        let tx_stage1 = tx.clone();
        let progress_future = async move {
            while let Some(event) = missing_files_rx.recv().await {
                tx_stage1
                    .send(DownloadStageEvent::FileSizeVerificationStage(event))
                    .ok();
            }
        };
        let (failed_files, _) = tokio::join!(missing_files_fut, progress_future);
        let missing_files: Vec<DepotFile> =
            failed_files.into_iter().map(|(file, _)| file).collect();

        // Check if there is space available on disk
        let required_space = missing_files
            .iter()
            .map(|depot_file| depot_file.size().unwrap_or(0))
            .sum::<u64>();

        path_resolver.check_free_space(required_space)?;

        // File allocation step
        let (files_allocation_tx, mut files_allocation_rx) = mpsc::unbounded_channel();
        let files_allocation_fut =
            self.allocate_missing_files(missing_files, path_resolver.clone(), files_allocation_tx);
        let tx_stage2 = tx.clone();
        let progress_future = async move {
            while let Some(event) = files_allocation_rx.recv().await {
                tx_stage2
                    .send(DownloadStageEvent::FileAllocationStage(event))
                    .ok();
            }
        };
        let (files_allocation_error, _) = tokio::join!(files_allocation_fut, progress_future);

        // Check if all files were allocated successfully, if not, we may have run out of disk space
        if !files_allocation_error.is_empty() {
            tx.send(DownloadStageEvent::FileAllocationError()).ok();
            return Err(DownloadError::FileAllocationError);
        }
        drop(files_allocation_error);

        // Download step

        let (files_download_tx, mut files_download_rx) = mpsc::unbounded_channel();
        let download_units = DownloadUnit::from_product_bundles(bundles);
        let downloader_future =
            self.download_files(download_units, &path_resolver, files_download_tx);
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

        let download_units = DownloadUnit::from_product_bundles(bundles);

        let missing_units = self
            .verify_download_units(download_units, path_resolver.clone(), tx)
            .await;

        let missing_units_count = missing_units.len();

        if missing_units_count > 0 {
            return Err(DownloadError::ChunkIntegrityCheckFailed(
                missing_units_count,
            ));
        }

        Ok(())
    }

    /// Transfers `download_units`, `self.threads` at a time.
    ///
    /// Every unit runs inside this future (`try_for_each_concurrent`), never
    /// under `tokio::spawn`. That is what makes dropping `download_game` or
    /// `repair_game` cancel the whole transfer: a spawned task would outlive
    /// the drop and keep writing into the install. The cancel contract in
    /// those methods' rustdoc depends on it, so keep it that way.
    async fn download_files(
        &self,
        download_units: Vec<DownloadUnit>,
        path_resolver: &PathResolver,
        tx: mpsc::UnboundedSender<DownloadEvent>,
    ) -> Result<(), DownloadError> {
        tx.send(DownloadEvent::Preparing).ok();

        tx.send(DownloadEvent::Prepared).ok();
        stream::iter(download_units)
            .map(Ok::<DownloadUnit, DownloadError>)
            .try_for_each_concurrent(self.threads, |download_unit| {
                tx.send(DownloadEvent::Downloading).ok();
                let tx = tx.clone();
                async move {

                    for attempt in 0..MAX_ATTEMPTS {

                        let secure_links_manager = &self.secure_links.clone();
                        let mut progress = ProgressGuard::new(tx.clone());

                        let file = path_resolver.open_file(&download_unit.path, true).await?;

                        let offset_writer =
                            OffsetWriter::new(file, download_unit.offset, download_unit.size)
                                .await
                                .map_err(DownloadError::DeflateError)?;
                        let sink =
                            HashingWriter::new(BufWriter::with_capacity(1024 * 1024, offset_writer));
                        let decoder_slot = Mutex::new(ZlibDecoder::new(sink));

                        let links = match secure_links_manager.get_secure_links(&download_unit.product_id).await {
                            Ok(links) => links,
                            // Retrying can't change these.
                            Err(
                                err @ (SecureLinksError::IncorrectGameId(..)
                                | SecureLinksError::ProductNotOwned(_)),
                            ) => return Err(DownloadError::SecureLinksError(err)),
                            Err(err) => {
                                secure_links_manager.invalidate_secure_links(&download_unit.product_id).await;
                                retry_or_return(attempt, MAX_ATTEMPTS, DownloadError::SecureLinksError(err)).await?;
                                continue;
                            },
                        };

                        let url_format = match links.get_highest_priority_url() {
                            Ok(url_format) => url_format,
                            Err(err) => {
                                secure_links_manager.invalidate_secure_links(&download_unit.product_id).await;
                                retry_or_return(attempt, MAX_ATTEMPTS, DownloadError::SecureLinksError(err)).await?;
                                continue;
                            },
                        };

                        let url = match download_unit.file_type {
                            FileType::DepotFile => url_format.parse_url(&download_unit.compressed_md5),
                            FileType::Other => url_format.parse_url_redist(&download_unit.compressed_md5),
                        };

                        let decoder_ref = &decoder_slot;
                        let stream_result = self.client
                            .stream_chunk(&url, |chunk: Bytes| {
                                progress.report(chunk.len());
                                let decoder = decoder_ref;
                                Box::pin(async move { decoder.lock().await.write_all(&chunk).await })
                            })
                            .await;
                        match stream_result {
                                Ok(_) => {},
                                Err(ClientError::HttpError { status, body }) => {
                                    let err = DownloadError::ClientError(ClientError::HttpError { status, body });
                                    if status == reqwest::StatusCode::UNAUTHORIZED {
                                        secure_links_manager.invalidate_secure_links(&download_unit.product_id).await;
                                        retry_now_or_return(attempt, MAX_ATTEMPTS, err)?;
                                    } else {
                                        retry_or_return(attempt, MAX_ATTEMPTS, err).await?;
                                    }
                                    continue;
                                }
                                Err(ClientError::ChunkStreamCallbackError(err)) => {
                                    return Err(DownloadError::ClientError(ClientError::ChunkStreamCallbackError(err)))
                                }
                                Err(ClientError::UrlParseError(err)) => {
                                    return Err(DownloadError::ClientError(ClientError::UrlParseError(err)))
                                }
                                Err(err) => {
                                    retry_or_return(attempt, MAX_ATTEMPTS, DownloadError::ClientError(err)).await?;
                                    continue;
                                }
                            };

                        let mut decoder = decoder_slot.into_inner();
                        if let Err(err) = decoder.shutdown().await {
                            return Err(DownloadError::DeflateError(err))
                        }

                        let (buf_writer, actual_md5) = decoder.into_inner().into_parts();
                        let writer = buf_writer.into_inner();

                        if actual_md5 != download_unit.md5 {
                            secure_links_manager.invalidate_secure_links(&download_unit.product_id).await;
                            retry_or_return(attempt, MAX_HASH_ATTEMPTS, DownloadError::ChunkHashMismatch {
                                path: download_unit.path.clone(),
                                offset: download_unit.offset,
                                expected: download_unit.md5.clone(),
                                actual: actual_md5,
                            }).await?;
                            continue;
                        }

                        if writer.remaining() != 0 {
                            secure_links_manager.invalidate_secure_links(&download_unit.product_id).await;
                            retry_or_return(attempt, MAX_ATTEMPTS, DownloadError::DeflateError(std::io::Error::new(
                                std::io::ErrorKind::InvalidData,
                                format!(
                                    "chunk verification failed for '{}' at offset {} (short by {} byte(s), checksum {})",
                                    download_unit.path,
                                    download_unit.offset,
                                    writer.remaining(),
                                    actual_md5
                                ),
                            ))).await?;
                            continue;
                        }
                        progress.commit();
                        break;
                    }
                    Ok(())
                }
            })
            .await?;
        Ok(())
    }

    async fn allocate_missing_files(
        &self,
        files: Vec<DepotFile>,
        path_resolver: Arc<PathResolver>,
        tx: mpsc::UnboundedSender<FileAllocationEvent>,
    ) -> Vec<DepotFile> {
        let failed_files = stream::iter(files)
            .map(|file| {
                let path_resolver = path_resolver.clone();
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

        failed_files
            .iter()
            .filter(|&unit| unit.is_some())
            // Unwrap is safe here because we know the unit is Some(_)
            .map(|unit| unit.clone().unwrap().clone())
            .collect::<Vec<_>>()
    }

    /// Returns the files that failed, each with the length it had on disk
    /// (`Some(0)` if absent, `None` if that couldn't be read).
    async fn verify_files_size(
        &self,
        files: Vec<DepotFile>,
        path_resolver: Arc<PathResolver>,
        tx: mpsc::UnboundedSender<FileSizeVerificationEvent>,
    ) -> Vec<(DepotFile, Option<u64>)> {
        let missing_files = stream::iter(files)
            .map(|file| {
                let path_resolver = path_resolver.clone();
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
                            return Some((file, None));
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
                            return Some((file, Some(0)));
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
                            return Some((file, None));
                        }
                    };

                    if file_size != expected_file_size {
                        tx.send(FileSizeVerificationEvent::FileSizeMismatch(
                            file.path.clone(),
                            expected_file_size,
                        ))
                        .ok();
                        return Some((file, Some(file_size)));
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

        missing_files.into_iter().flatten().collect()
    }

    async fn verify_download_units(
        &self,
        download_units: Vec<DownloadUnit>,
        path_resolver: Arc<PathResolver>,
        tx: mpsc::UnboundedSender<VerificationEvent>,
    ) -> Vec<DownloadUnit> {
        let units = stream::iter(download_units)
            .map(|download_unit| {
                let path_resolver = path_resolver.clone();
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

                    #[cfg(test)]
                    self.checksums
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let actual_checksum = match compute_chunk_checksum(
                        final_path,
                        download_unit.offset,
                        download_unit.size,
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
                    None
                }
            })
            .buffer_unordered(self.threads)
            .collect::<Vec<_>>()
            .await;

        units
            .iter()
            .filter(|&unit| unit.is_some())
            .map(|unit| unit.clone().unwrap().clone())
            .collect::<Vec<_>>()
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tokio::sync::Notify;

    use super::*;
    use crate::{
        depot::Chunk,
        games::GamesManager,
        secure_links::{CdnUrlParams, SecureLinks, UrlFormat},
        test_support::{ChunkServer, Reply, TempDir, md5_hex, zlib},
    };

    const PRODUCT: &str = "1";

    /// The path `download_files` asks the server for, given a unit's
    /// compressed MD5 (see the `url_format` in `harness`).
    fn chunk_path(compressed_md5: &str) -> String {
        format!(
            "/depot/{}/{}/{}",
            &compressed_md5[0..2],
            &compressed_md5[2..4],
            compressed_md5
        )
    }

    /// One file of `raw` in the temp dir and a unit that covers all of it.
    /// `n` makes the compressed MD5 (and so the URL) distinct per unit.
    fn unit(dir: &TempDir, n: u8, raw: &[u8], product_id: &str) -> DownloadUnit {
        let path = format!("file{n}.bin");
        std::fs::write(dir.path().join(&path), vec![0u8; raw.len()]).unwrap();
        DownloadUnit {
            md5: md5_hex(raw),
            size: raw.len() as u64,
            compressed_md5: format!("{n:02x}{}", "ab".repeat(15)),
            _compressed_size: 0,
            path,
            offset: 0,
            file_type: FileType::DepotFile,
            product_id: product_id.to_string(),
        }
    }

    struct Harness {
        server: ChunkServer,
        dir: TempDir,
        downloader: Downloader,
    }

    /// A downloader whose secure links for `PRODUCT` point at a fresh local
    /// server, and whose owned-products cache already says `PRODUCT` is
    /// owned, so nothing leaves the machine.
    async fn harness(threads: usize) -> Harness {
        let server = ChunkServer::start().await;
        let client = HttpClient::new_with_client(reqwest::Client::new());
        let games = GamesManager::new(client.clone());
        games.inner.lock().await.owned_products.owned = vec![PRODUCT.parse().unwrap()];
        let secure_links = SecureLinksManager::new(client.clone(), games);
        secure_links.fixture_links(
            PRODUCT,
            SecureLinks {
                product_id: 1,
                urls: vec![UrlFormat {
                    endpoint_name: "test".into(),
                    url_format: "{base_url}{path}".into(),
                    priority: 1,
                    parameters: CdnUrlParams {
                        base_url: server.base_url(),
                        path: "/depot".into(),
                        token: String::new(),
                        expires_at: None,
                        dirs: None,
                        ttl: None,
                        source: None,
                        gog_token: None,
                        l: None,
                    },
                }],
            },
        );
        let mut downloader = Downloader::new(client, secure_links);
        downloader.threads = threads;
        Harness {
            server,
            dir: TempDir::new(),
            downloader,
        }
    }

    /// Runs `download_files` and returns its result, the net bytes reported
    /// (`Progress` minus `ProgressRegression`) and the gross `Progress` sum.
    async fn run(h: &Harness, units: Vec<DownloadUnit>) -> (Result<(), DownloadError>, i64, i64) {
        let resolver = PathResolver::new(h.dir.path().to_path_buf()).await.unwrap();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let result = h.downloader.download_files(units, &resolver, tx).await;
        let (mut net, mut gross) = (0i64, 0i64);
        while let Some(event) = rx.recv().await {
            match event {
                DownloadEvent::Progress(n) => {
                    net += n as i64;
                    gross += n as i64;
                }
                DownloadEvent::ProgressRegression(n) => net -= n as i64,
                _ => {}
            }
        }
        (result, net, gross)
    }

    /// Fixture units share one product, so a harness needs the dir before
    /// the units exist; this builds both.
    async fn one_unit(threads: usize, raw: &[u8]) -> (Harness, DownloadUnit) {
        let h = harness(threads).await;
        let u = unit(&h.dir, 1, raw, PRODUCT);
        (h, u)
    }

    #[tokio::test(start_paused = true)]
    async fn transport_failures_then_bad_md5_on_the_last_attempt_is_an_error() {
        let (h, u) = one_unit(1, b"the right bytes").await;
        let path = chunk_path(&u.compressed_md5);
        let mut script = vec![Reply::Close; 5];
        script.push(Reply::Body(zlib(b"the wrong bytes")));
        h.server.script(&path, script);

        let (result, net, _) = run(&h, vec![u]).await;

        assert!(
            matches!(result, Err(DownloadError::ChunkHashMismatch { .. })),
            "{result:?}"
        );
        assert_eq!(h.server.requests(&path), MAX_ATTEMPTS as usize);
        assert_eq!(net, 0);
    }

    #[tokio::test(start_paused = true)]
    async fn bad_md5_is_only_retried_on_the_first_two_attempts() {
        // Pins `MAX_HASH_ATTEMPTS`: a mismatch on the third attempt
        // is terminal even though attempts remain.
        let (h, u) = one_unit(1, b"the right bytes").await;
        let path = chunk_path(&u.compressed_md5);
        h.server
            .script(&path, vec![Reply::Body(zlib(b"the wrong bytes"))]);

        let (result, net, _) = run(&h, vec![u]).await;

        assert!(
            matches!(result, Err(DownloadError::ChunkHashMismatch { .. })),
            "{result:?}"
        );
        assert_eq!(h.server.requests(&path), 3);
        assert_eq!(net, 0);
    }

    #[tokio::test(start_paused = true)]
    async fn every_attempt_failing_is_an_error() {
        let (h, u) = one_unit(1, b"the right bytes").await;
        let path = chunk_path(&u.compressed_md5);
        h.server.script(&path, vec![Reply::Status(500)]);

        let (result, net, _) = run(&h, vec![u]).await;

        assert!(
            matches!(
                result,
                Err(DownloadError::ClientError(ClientError::HttpError { status, .. }))
                    if status == reqwest::StatusCode::INTERNAL_SERVER_ERROR
            ),
            "{result:?}"
        );
        assert_eq!(h.server.requests(&path), MAX_ATTEMPTS as usize);
        assert_eq!(net, 0);
    }

    #[tokio::test(start_paused = true)]
    async fn bad_md5_then_good_bytes_succeeds_and_nets_one_chunk_of_progress() {
        let raw = b"the right bytes";
        let (h, u) = one_unit(1, raw).await;
        let path = chunk_path(&u.compressed_md5);
        let good = zlib(raw);
        h.server.script(
            &path,
            vec![
                Reply::Body(zlib(b"the wrong bytes")),
                Reply::Body(good.clone()),
            ],
        );
        let file = h.dir.path().join(&u.path);

        let (result, net, gross) = run(&h, vec![u]).await;

        result.unwrap();
        assert_eq!(h.server.requests(&path), 2);
        // Progress counts compressed bytes; the bad attempt was taken back.
        assert_eq!(net, good.len() as i64);
        assert!(gross > net, "the first attempt's bytes were reported too");
        assert_eq!(std::fs::read(file).unwrap(), raw);
    }

    #[tokio::test]
    async fn a_terminal_failure_aborts_the_batch_and_nets_progress_to_zero() {
        let h = harness(2).await;
        let raw = b"a chunk that never finishes downloading";
        let stuck = unit(&h.dir, 1, raw, PRODUCT);
        let failing = unit(&h.dir, 2, raw, PRODUCT);
        let half_sent = Arc::new(Notify::new());
        let body = zlib(raw);
        h.server.script(
            &chunk_path(&stuck.compressed_md5),
            vec![Reply::PartialThenHang {
                sent: body.len() / 2,
                body,
                notify: half_sent.clone(),
            }],
        );
        // Garbage isn't zlib, so the decoder rejects it: a terminal
        // `ChunkStreamCallbackError`, no retry. Held back until the stuck
        // unit has sent its half, plus a beat for the client to read it.
        h.server.script(
            &chunk_path(&failing.compressed_md5),
            vec![Reply::After(
                half_sent,
                Box::new(Reply::Delay(
                    Duration::from_millis(100),
                    Box::new(Reply::Body(b"not zlib".to_vec())),
                )),
            )],
        );

        let (result, net, gross) =
            tokio::time::timeout(Duration::from_secs(5), run(&h, vec![stuck, failing]))
                .await
                .expect("the batch must return without waiting for the stuck unit");

        assert!(
            matches!(
                result,
                Err(DownloadError::ClientError(
                    ClientError::ChunkStreamCallbackError(_)
                ))
            ),
            "{result:?}"
        );
        assert!(gross > 0, "the stuck unit had reported progress");
        assert_eq!(net, 0, "cancelled units take their progress back");
    }

    #[tokio::test(start_paused = true)]
    async fn product_not_owned_returns_without_retrying() {
        let h = harness(1).await;
        let u = unit(&h.dir, 1, b"whatever", "2");
        let start = tokio::time::Instant::now();

        let (result, net, _) = run(&h, vec![u]).await;

        assert!(
            matches!(
                result,
                Err(DownloadError::SecureLinksError(
                    SecureLinksError::ProductNotOwned(_)
                ))
            ),
            "{result:?}"
        );
        assert_eq!(h.downloader.secure_links.lookups(), 1);
        assert_eq!(start.elapsed(), Duration::ZERO, "no backoff");
        assert_eq!(net, 0);
    }

    #[tokio::test(start_paused = true)]
    async fn incorrect_game_id_returns_without_retrying() {
        let h = harness(1).await;
        let u = unit(&h.dir, 1, b"whatever", "not-a-number");
        let start = tokio::time::Instant::now();

        let (result, net, _) = run(&h, vec![u]).await;

        assert!(
            matches!(
                result,
                Err(DownloadError::SecureLinksError(
                    SecureLinksError::IncorrectGameId(..)
                ))
            ),
            "{result:?}"
        );
        assert_eq!(h.downloader.secure_links.lookups(), 1);
        assert_eq!(start.elapsed(), Duration::ZERO, "no backoff");
        assert_eq!(net, 0);
    }

    #[tokio::test]
    async fn repair_after_a_dropped_download_completes_the_install() {
        let h = harness(1).await;
        let a = b"the first chunk of the file, done before the drop";
        let b = b"the second chunk, cut off by the drop";
        let chunk = |n: u8, raw: &[u8]| Chunk {
            md5: md5_hex(raw),
            size: raw.len() as u64,
            compressed_md5: format!("{n:02x}{}", "ab".repeat(15)),
            compressed_size: zlib(raw).len() as u64,
        };
        let (chunk_a, chunk_b) = (chunk(1, a), chunk(2, b));
        let (path_a, path_b) = (
            chunk_path(&chunk_a.compressed_md5),
            chunk_path(&chunk_b.compressed_md5),
        );
        let bundle = || ProductBundle {
            product_id: PRODUCT.to_string(),
            product_files: vec![DepotFile {
                md5: None,
                path: "game.bin".to_string(),
                chunks: Some(vec![chunk_a.clone(), chunk_b.clone()]),
                file_type: "DepotFile".to_string(),
            }],
        };
        let file = h.dir.path().join("game.bin");
        std::fs::write(&file, vec![0u8; a.len() + b.len()]).unwrap();

        h.server.script(&path_a, vec![Reply::Body(zlib(a))]);
        let half_sent = Arc::new(Notify::new());
        let body_b = zlib(b);
        h.server.script(
            &path_b,
            vec![
                Reply::PartialThenHang {
                    sent: body_b.len() / 2,
                    body: body_b.clone(),
                    notify: half_sent.clone(),
                },
                Reply::Body(body_b),
            ],
        );

        // Drop the transfer once the second chunk is half received.
        let resolver = PathResolver::new(h.dir.path().to_path_buf()).await.unwrap();
        let (tx, _rx) = mpsc::unbounded_channel();
        let download = h.downloader.download_files(
            DownloadUnit::from_product_bundles(vec![bundle()]),
            &resolver,
            tx,
        );
        tokio::time::timeout(Duration::from_secs(5), async {
            tokio::select! {
                result = download => panic!("the download finished: {result:?}"),
                _ = half_sent.notified() => {}
            }
        })
        .await
        .expect("the second chunk never started");

        let (tx, mut rx) = mpsc::unbounded_channel();
        h.downloader
            .repair(vec![bundle()], h.dir.path().to_str().unwrap(), tx)
            .await
            .unwrap();

        assert_eq!(std::fs::read(&file).unwrap(), [a.as_slice(), b].concat());
        assert_eq!(h.server.requests(&path_a), 1, "a verified chunk is kept");
        assert_eq!(h.server.requests(&path_b), 2);
        let (mut verified, mut mismatched) = (0, 0);
        while let Ok(event) = rx.try_recv() {
            match event {
                DownloadStageEvent::VerificationStage(VerificationEvent::Verified(..)) => {
                    verified += 1
                }
                DownloadStageEvent::VerificationStage(VerificationEvent::ChecksumMismatch(..)) => {
                    mismatched += 1
                }
                _ => {}
            }
        }
        assert_eq!((verified, mismatched), (1, 1));
    }

    fn chunk_of(n: u8, raw: &[u8]) -> Chunk {
        Chunk {
            md5: md5_hex(raw),
            size: raw.len() as u64,
            compressed_md5: format!("{n:02x}{}", "ab".repeat(15)),
            compressed_size: zlib(raw).len() as u64,
        }
    }

    fn bundle_of(files: Vec<(&str, Vec<Chunk>)>) -> ProductBundle {
        ProductBundle {
            product_id: PRODUCT.to_string(),
            product_files: files
                .into_iter()
                .map(|(path, chunks)| DepotFile {
                    md5: None,
                    path: path.to_string(),
                    chunks: Some(chunks),
                    file_type: "DepotFile".to_string(),
                })
                .collect(),
        }
    }

    /// Runs `repair` and returns the `(verified, mismatched)` counts of its
    /// verification events.
    async fn repair_counts(h: &Harness, bundle: ProductBundle) -> (usize, usize) {
        let (tx, mut rx) = mpsc::unbounded_channel();
        h.downloader
            .repair(vec![bundle], h.dir.path().to_str().unwrap(), tx)
            .await
            .unwrap();
        let (mut verified, mut mismatched) = (0, 0);
        while let Ok(event) = rx.try_recv() {
            match event {
                DownloadStageEvent::VerificationStage(VerificationEvent::Verified(..)) => {
                    verified += 1
                }
                DownloadStageEvent::VerificationStage(VerificationEvent::ChecksumMismatch(..)) => {
                    mismatched += 1
                }
                _ => {}
            }
        }
        (verified, mismatched)
    }

    #[tokio::test]
    async fn repair_over_an_empty_directory_hashes_nothing() {
        let h = harness(2).await;
        let (a, b, c) = (b"first chunk", b"second chunk", b"another file");
        let chunks = [chunk_of(1, a), chunk_of(2, b), chunk_of(3, c)];
        for (chunk, raw) in chunks.iter().zip([a.as_slice(), b, c]) {
            h.server.script(
                &chunk_path(&chunk.compressed_md5),
                vec![Reply::Body(zlib(raw))],
            );
        }
        let bundle = bundle_of(vec![
            ("game.bin", vec![chunks[0].clone(), chunks[1].clone()]),
            ("data/other.bin", vec![chunks[2].clone()]),
        ]);

        let counts = repair_counts(&h, bundle).await;

        assert_eq!(
            counts,
            (0, 3),
            "one event per unit, as when they were hashed"
        );
        assert_eq!(
            h.downloader
                .checksums
                .load(std::sync::atomic::Ordering::Relaxed),
            0
        );
        assert_eq!(
            std::fs::read(h.dir.path().join("game.bin")).unwrap(),
            [a.as_slice(), b].concat()
        );
        assert_eq!(
            std::fs::read(h.dir.path().join("data/other.bin")).unwrap(),
            c
        );
    }

    #[tokio::test]
    async fn repair_hashes_only_below_a_files_old_length() {
        let h = harness(1).await;
        let (a, b) = (
            b"the chunk that is already on disk",
            b"the chunk past the end",
        );
        let (chunk_a, chunk_b) = (chunk_of(1, a), chunk_of(2, b));
        let path_a = chunk_path(&chunk_a.compressed_md5);
        let path_b = chunk_path(&chunk_b.compressed_md5);
        h.server.script(&path_b, vec![Reply::Body(zlib(b))]);
        // A shorter file: its size mismatches, but its first chunk is good.
        let file = h.dir.path().join("game.bin");
        std::fs::write(&file, a).unwrap();

        let counts = repair_counts(&h, bundle_of(vec![("game.bin", vec![chunk_a, chunk_b])])).await;

        assert_eq!(counts, (1, 1));
        assert_eq!(
            h.downloader
                .checksums
                .load(std::sync::atomic::Ordering::Relaxed),
            1
        );
        assert_eq!(h.server.requests(&path_a), 0);
        assert_eq!(h.server.requests(&path_b), 1);
        assert_eq!(std::fs::read(&file).unwrap(), [a.as_slice(), b].concat());
    }
}
