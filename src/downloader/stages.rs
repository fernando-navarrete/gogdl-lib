//! Per-stage filesystem/network work for a download job: allocating files
//! on disk, verifying existing files/chunks, and downloading chunks. Each
//! stage is driven by `Downloader`'s orchestration methods in
//! `downloader.rs`, which build one `PathResolver` per job and share it
//! across every stage call here.

use std::{path::PathBuf, sync::Arc, thread, time::Duration};

use futures::{StreamExt, stream};
use tokio::{fs, sync::mpsc};

use crate::{
    client::HttpClient,
    depot::{DepotFile, DownloadUnit},
    downloader::{
        DownloadError,
        adaptive::{AdaptiveLimiter, ThroughputMeter},
        config::DownloadConfig,
        downloader::Downloader,
        events::{DownloadEvent, FileAllocationEvent, FileVerifyEvent, VerifyChunksEvent},
        stream::{UnitAttemptError, stream_unit_to_file},
        util::{ChecksumAlgorithm, PathResolver, compute_chunk_checksum},
    },
    secure_links::UrlFormat,
};

/// The default fan-out for the CPU-bound verification/allocation stages: the
/// number of logical CPUs, falling back to 4 if that can't be determined.
/// The network-bound chunk-download stage does **not** use this — see
/// `downloader::adaptive` for why CPU count is the wrong proxy for network
/// parallelism.
pub(crate) fn default_concurrency() -> usize {
    thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
}

/// Returns a pseudo-random jitter in `[0, max_ms)`, used to spread out retry
/// attempts across concurrently-retrying chunks so they don't all hammer the
/// CDN again at exactly the same instant (thundering herd). Not
/// cryptographic — just needs to vary run to run, so the current time's
/// sub-second component is enough and avoids pulling in a `rand` dependency.
fn jitter_ms(max_ms: u64) -> u64 {
    if max_ms == 0 {
        return 0;
    }
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    nanos as u64 % max_ms
}

/// Computes the delay before retry attempt `attempt` (1-based: the delay
/// before the 2nd, 3rd, ... try). Exponential backoff off `config.base_backoff`,
/// capped at 30s, plus up to 50% jitter.
fn backoff_delay(config: &DownloadConfig, attempt: usize) -> Duration {
    let exponent = (attempt - 1).min(16) as u32;
    let base_ms = (config.base_backoff.as_millis() as u64).saturating_mul(1u64 << exponent);
    let capped_ms = base_ms.min(30_000);
    let jitter = jitter_ms(capped_ms / 2 + 1);
    Duration::from_millis(capped_ms + jitter)
}

/// The per-job plumbing every chunk download needs — bundled so the download
/// stage clones one handle per product/unit instead of threading nine
/// separate arguments through `download_chunk`. Every field is a cheap
/// clone (`Arc`s and channel/handle clones).
#[derive(Clone)]
pub(crate) struct DownloadStageCtx {
    pub(crate) client: HttpClient,
    pub(crate) resolver: Arc<PathResolver>,
    pub(crate) limiter: Arc<AdaptiveLimiter>,
    pub(crate) meter: Arc<ThroughputMeter>,
    pub(crate) config: Arc<DownloadConfig>,
    pub(crate) tx: mpsc::UnboundedSender<DownloadEvent>,
}

/// Why a single chunk failed verification. Shared by `verify_chunks`
/// (repair) and `Downloader::verify`, which map the same underlying checks
/// onto their respective event enums — the resolved on-disk path is carried
/// where a variant's event payload needs it.
pub(crate) enum ChunkVerifyIssue {
    PathResolveError(std::io::Error),
    FileNotFound,
    ChecksumCalculationError(PathBuf, std::io::Error),
    ChecksumMismatch(PathBuf),
}

/// The one resolve → hash → compare ladder behind every chunk-level
/// verification: resolves the unit's path under the install root, MD5s the
/// chunk's byte range, and compares against the manifest checksum.
pub(crate) async fn check_unit_checksum(
    resolver: &PathResolver,
    unit: &DownloadUnit,
) -> Result<(), ChunkVerifyIssue> {
    let path = match resolver.resolve_existing_path(&unit.path).await {
        Ok(Some(path)) => path,
        Ok(None) => return Err(ChunkVerifyIssue::FileNotFound),
        Err(e) => return Err(ChunkVerifyIssue::PathResolveError(e)),
    };

    let actual_checksum =
        match compute_chunk_checksum(path.clone(), unit.offset, unit.size, ChecksumAlgorithm::Md5)
            .await
        {
            Ok(checksum) => checksum,
            Err(e) => return Err(ChunkVerifyIssue::ChecksumCalculationError(path, e)),
        };

    if actual_checksum != unit.md5 {
        return Err(ChunkVerifyIssue::ChecksumMismatch(path));
    }
    Ok(())
}

impl Downloader {
    pub(crate) async fn download_chunk(
        &self,
        units: Vec<DownloadUnit>,
        product_id: &str,
        ctx: DownloadStageCtx,
        is_dependency: bool,
    ) -> Result<(), DownloadError> {
        let secure_links = self.secure_links.get_secure_links(product_id).await?;
        // Every known CDN endpoint for this product, highest-priority first.
        // Cloned once up front (cheap: a handful of small structs) so units
        // below round-robin across them without re-fetching secure links.
        let endpoints = secure_links
            .get_prioritized_urls()
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();

        // This only bounds how many units are *polled* at once; the actual,
        // adaptive concurrency limit is `ctx.limiter`, acquired per-unit
        // below. It needs to be at least `config.max_concurrency` so the
        // limiter is never the thing left idle waiting for more units to
        // poll.
        let poll_bound = ctx.config.max_concurrency.max(1);

        stream::iter(units.into_iter().enumerate())
            .map(|(index, unit)| {
                let ctx = ctx.clone();
                let endpoints = endpoints.clone();
                let auth = self.auth.clone();
                async move {
                    // Acquired before any I/O so a chunk waiting on a full
                    // limiter doesn't hold a file handle or auth token while
                    // parked.
                    let _permit = ctx.limiter.acquire().await;

                    if endpoints.is_empty() {
                        println!(
                            "[SECURE_LINK_MISSING] {} ({}): no secure link available",
                            unit.path.clone(),
                            unit.file_type
                        );
                        ctx.tx
                            .send(DownloadEvent::SecureLinkError(unit.path.clone()))
                            .ok();
                        return;
                    }

                    let access_token = match auth.get_auth().await {
                        Some(auth) => auth.access_token,
                        None => {
                            println!(
                                "[NOT_AUTHENTICATED] {} ({})",
                                unit.path.clone(),
                                unit.file_type
                            );
                            ctx.tx
                                .send(DownloadEvent::DownloadError(unit.path.clone()))
                                .ok();
                            return;
                        }
                    };

                    let file_path = match ctx.resolver.resolve_path(&unit.path).await {
                        Ok(file_path) => file_path,
                        Err(e) => {
                            println!(
                                "[PATH_RESOLVE_ERROR] {} ({}): {}",
                                unit.path.clone(),
                                unit.file_type,
                                e
                            );
                            ctx.tx
                                .send(DownloadEvent::PathResolveError(unit.path.clone()))
                                .ok();
                            return;
                        }
                    };

                    let mut file = match fs::OpenOptions::new().write(true).open(&file_path).await {
                        Ok(file) => file,
                        Err(e) => {
                            println!(
                                "[FILE_OPEN_ERROR] {} ({}): {}",
                                unit.path.clone(),
                                unit.file_type,
                                e
                            );
                            ctx.tx
                                .send(DownloadEvent::WriteError(unit.path.clone()))
                                .ok();
                            return;
                        }
                    };

                    download_unit_with_retries(
                        &ctx,
                        &endpoints,
                        index,
                        &unit,
                        access_token,
                        &mut file,
                        is_dependency,
                    )
                    .await;
                }
            })
            .buffer_unordered(poll_bound)
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
                    match check_unit_checksum(&resolver, &unit).await {
                        Ok(()) => None,
                        Err(issue) => {
                            match &issue {
                                ChunkVerifyIssue::PathResolveError(e) => {
                                    println!(
                                        "Failed to resolve path for unit {}: {}",
                                        unit.path, e
                                    );
                                    tx.send(VerifyChunksEvent::PathResolveError(unit.path.clone()))
                                        .ok();
                                }
                                ChunkVerifyIssue::FileNotFound => {
                                    println!("File not found for unit {}", unit.path);
                                    tx.send(VerifyChunksEvent::FileNotFound(unit.path.clone()))
                                        .ok();
                                }
                                ChunkVerifyIssue::ChecksumCalculationError(_, e) => {
                                    tx.send(VerifyChunksEvent::ChecksumCalculationError(
                                        unit.path.clone(),
                                    ))
                                    .ok();
                                    println!("ERROR: {}: {}", unit.path.clone(), e);
                                }
                                ChunkVerifyIssue::ChecksumMismatch(_) => {
                                    tx.send(VerifyChunksEvent::ChunkChecksumMismatch(
                                        unit.path.clone(),
                                    ))
                                    .ok();
                                    println!("ERROR: {}: checksum mismatch", unit.path.clone());
                                }
                            }
                            Some(unit)
                        }
                    }
                }
            })
            .buffer_unordered(concurrency)
            .collect::<Vec<_>>()
            .await;

        Ok(results.into_iter().flatten().collect())
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
                            println!("ERROR: {} ({}): {}", file.path.clone(), file.file_type, err);
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
            println!("FAILED TO ALLOCATE: {} ({})", file.path, file.file_type);
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
                            println!("ERROR: {} ({}): {}", file.path.clone(), file.file_type, err);
                            return Some(file.clone());
                        }
                    };
                    let final_path = match opt_path {
                        Some(path) => path,
                        None => {
                            tx.send(FileVerifyEvent::FileNotFound(file.path.clone()))
                                .ok();
                            println!(
                                "ERROR: {} ({}): FILE NOT FOUND",
                                file.path.clone(),
                                file.file_type
                            );
                            return Some(file.clone());
                        }
                    };

                    let size = match resolver.get_file_size(&final_path).await {
                        Ok(size) => size,
                        Err(e) => {
                            tx.send(FileVerifyEvent::CouldNotReadFileSize(file.path.clone()))
                                .ok();
                            println!("ERROR: {} ({}): {}", file.path.clone(), file.file_type, e);
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
                            "ERROR: {} ({}): size mismatch: {} != {}",
                            file.path.clone(),
                            file.file_type,
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

        Ok(results.into_iter().flatten().collect())
    }
}

/// The bounded retry loop for one unit: round-robins attempts across
/// `endpoints` (starting at `index` so concurrent chunks spread over hosts),
/// backs off exponentially between tries, falls back to the redist store on
/// primary failure for dependency depots, and reports the final outcome on
/// `ctx.tx`. Per-unit failures are events, never `Err` — one bad chunk
/// doesn't fail the job's `Result` (see api.md §5).
async fn download_unit_with_retries(
    ctx: &DownloadStageCtx,
    endpoints: &[UrlFormat],
    index: usize,
    unit: &DownloadUnit,
    access_token: String,
    file: &mut fs::File,
    is_dependency: bool,
) {
    let mut counted: u64 = 0;
    let mut download_err: Option<Box<dyn std::fmt::Display + Send>> = None;
    let mut last_primary_url = String::new();
    let mut last_redist_url = String::new();
    let max_attempts = ctx.config.max_retries.max(1);

    'attempts: for attempt in 1..=max_attempts {
        if attempt > 1 {
            tokio::time::sleep(backoff_delay(&ctx.config, attempt - 1)).await;
        }

        // Round-robin across known CDN endpoints (by unit index and attempt
        // number) so concurrent chunks, and successive retries of the same
        // chunk, spread across hosts instead of all hammering one.
        let endpoint = &endpoints[(index + attempt - 1) % endpoints.len()];
        let primary_url = endpoint.parse_url(&unit.compressed_md5);
        let redist_url = endpoint.parse_url_redist(&unit.compressed_md5);
        last_primary_url = primary_url.clone();

        let primary_result = stream_unit_to_file(
            &ctx.client,
            &primary_url,
            access_token.clone(),
            file,
            unit.offset,
            &mut counted,
            &ctx.tx,
            &ctx.meter,
            ctx.config.response_timeout,
            ctx.config.idle_timeout,
        )
        .await;

        let result = match primary_result {
            Ok(()) => Ok(()),
            Err(UnitAttemptError::Write(e)) => Err(UnitAttemptError::Write(e)),
            Err(UnitAttemptError::Download(err)) if is_dependency => {
                // The primary secure-link CDN failed for this dependency
                // chunk (e.g. a transient network error) — try the redist
                // store, which hosts dependency content under a different
                // path, before deciding whether the whole attempt failed.
                // Only dependency depots take this path: game content never
                // lives under the redist store, so falling back there for a
                // game chunk would just trade a retryable primary error for
                // a permanent 404 and stop the retry loop dead (see
                // download_chunk's module docs / the plan that introduced
                // this gate).
                println!(
                    "[PRIMARY_ATTEMPT_DOWNLOAD_ERROR] {}: {} (falling back to redist store)",
                    unit.path.clone(),
                    err
                );
                last_redist_url = redist_url.clone();
                let redist_result = stream_unit_to_file(
                    &ctx.client,
                    &redist_url,
                    access_token.clone(),
                    file,
                    unit.offset,
                    &mut counted,
                    &ctx.tx,
                    &ctx.meter,
                    ctx.config.response_timeout,
                    ctx.config.idle_timeout,
                )
                .await;
                match &redist_result {
                    Ok(()) => {}
                    Err(UnitAttemptError::Write(e)) => {
                        println!("[REDIST_ATTEMPT_WRITE_ERROR] {}: {}", unit.path.clone(), e);
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
            // Not a dependency depot (or the redist fallback above didn't
            // apply): let the primary error flow through as-is, so the
            // transient-retry check below sees the real (possibly
            // retryable) cause instead of a fallback's unrelated result.
            Err(UnitAttemptError::Download(err)) => Err(UnitAttemptError::Download(err)),
        };

        match result {
            Ok(()) => {
                ctx.tx
                    .send(DownloadEvent::ChunkDownloaded {
                        path: unit.path.clone(),
                    })
                    .ok();
                break 'attempts;
            }
            Err(UnitAttemptError::Write(e)) => {
                // Disk errors aren't retried: every attempt (primary or
                // redist) writes to the same local file, so a disk-side
                // failure will recur.
                println!(
                    "[FINAL_WRITE_ERROR] {} ({}): {} (primary: {}, redist: {})",
                    unit.path.clone(),
                    unit.file_type,
                    e,
                    last_primary_url,
                    last_redist_url
                );
                ctx.tx
                    .send(DownloadEvent::WriteError(unit.path.clone()))
                    .ok();
                return;
            }
            Err(UnitAttemptError::Download(err)) => {
                let transient = err.is_transient();
                if transient {
                    ctx.meter.add_transient_error();
                }
                let is_last = attempt == max_attempts;
                if !transient || is_last {
                    download_err = Some(Box::new(err));
                    break 'attempts;
                }
                println!(
                    "[RETRYABLE_ERROR] {} (try {}/{}): {}",
                    unit.path, attempt, max_attempts, err
                );
                // else: transient and attempts remain — loop around,
                // backing off before the next try.
            }
        }
    }

    if let Some(err) = download_err {
        println!(
            "[FINAL_DOWNLOAD_ERROR] {} ({}): {} (primary: {}, redist: {})",
            unit.path.clone(),
            unit.file_type,
            err,
            last_primary_url,
            last_redist_url
        );
        ctx.tx
            .send(DownloadEvent::DownloadError(unit.path.clone()))
            .ok();
    }
}
