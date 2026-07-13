//! The `Downloader` engine: orchestrates the multi-stage `download`,
//! `repair_download`, and `verify` jobs by driving the per-stage helpers in
//! `stages.rs` and forwarding their events, tagged by stage, to the
//! caller's channel.

use std::{collections::HashSet, future::Future, path::PathBuf, sync::Arc};

use crate::{
    DownloadableFiles,
    auth::AuthManager,
    client::HttpClient,
    downloader::{
        DownloadConfig, DownloadError,
        adaptive::{AdaptiveLimiter, ThroughputMeter, spawn_controller},
        events::{
            DownloadDetail, DownloadEvent, DownloadJobEvent, DownloadStage, RepairDetail,
            RepairEvent, RepairStage, VerifyEvent,
        },
        stages::default_concurrency,
        util::{ChecksumAlgorithm, PathResolver, compute_chunk_checksum},
    },
    secure_links::SecureLinksManager,
};
use futures::{StreamExt, stream};
use tokio::sync::mpsc;

pub struct Downloader {
    pub client: HttpClient,
    pub secure_links: SecureLinksManager,
    pub auth: AuthManager,
    pub config: DownloadConfig,
}
impl Downloader {
    /// Builds a `Downloader` with caller-supplied network tuning
    /// (concurrency bounds, timeouts, retry policy) — see `DownloadConfig`.
    /// `DownloadManager` (the only caller) always has a config on hand
    /// (defaulted or overridden via `set_download_config`), so there's no
    /// separate `new` that assumes `DownloadConfig::default()`.
    pub fn new_with_config(
        client: HttpClient,
        secure_links: SecureLinksManager,
        auth: AuthManager,
        config: DownloadConfig,
    ) -> Self {
        Self {
            client,
            secure_links,
            auth,
            config,
        }
    }
    pub async fn repair_download(
        &self,
        files: Vec<DownloadableFiles>,
        path: &str,
        tx: mpsc::UnboundedSender<RepairEvent>,
    ) -> Result<(), DownloadError> {
        let resolver = Arc::new(PathResolver::new(PathBuf::from(path)).await?);

        let all_files = files
            .clone()
            .into_iter()
            .flat_map(|f| f.product_files)
            .collect::<Vec<_>>();

        let verify_result = run_stage(
            &tx,
            |event| RepairEvent {
                stage: RepairStage::VerifyingFiles,
                detail: RepairDetail::FileVerify(event),
            },
            |stage_tx| self.verify_files(all_files.clone(), resolver.clone(), stage_tx),
        )
        .await?;

        run_stage(
            &tx,
            |event| RepairEvent {
                stage: RepairStage::Allocating,
                detail: RepairDetail::Allocation(event),
            },
            |stage_tx| self.allocate_files(verify_result.clone(), resolver.clone(), stage_tx),
        )
        .await?;

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

        let mut errored_units = run_stage(
            &tx,
            |event| RepairEvent {
                stage: RepairStage::VerifyingChunks,
                detail: RepairDetail::ChunkVerify(event),
            },
            |stage_tx| self.verify_chunks(resolver.clone(), download_units, stage_tx),
        )
        .await?; // <- This is the final list of chunks to download

        let missing_from_first_step = verify_result
            .iter()
            .flat_map(|depot_file| depot_file.get_download_units())
            .collect::<Vec<_>>();

        errored_units.extend(missing_from_first_step);

        let needs_download_map: HashSet<&str> =
            errored_units.iter().map(|f| f.path.as_str()).collect();

        let (total_bytes, total_chunks) = files
            .iter()
            .flat_map(|f| f.product_files.iter())
            .flat_map(|df| df.get_download_units())
            .filter(|u| needs_download_map.contains(&u.path.as_str()))
            .fold((0u64, 0usize), |(bytes, chunks), u| (bytes + u.size, chunks + 1));

        // One adaptive concurrency limit shared by every product's chunks in
        // this job — not a fixed, CPU-derived number per product (see
        // `downloader::adaptive`). The controller hill-climbs it against
        // measured throughput for the lifetime of the download stage below.
        let meter = ThroughputMeter::new();
        let limiter = AdaptiveLimiter::new(self.config.min_concurrency);
        let controller = spawn_controller(meter.clone(), limiter.clone(), self.config.clone());
        let config = Arc::new(self.config.clone());

        let results = run_stage(
            &tx,
            |event| RepairEvent {
                stage: RepairStage::Downloading,
                detail: RepairDetail::Download(event),
            },
            |stage_tx| {
                stage_tx
                    .send(DownloadEvent::Total {
                        bytes: total_bytes,
                        chunks: total_chunks,
                    })
                    .ok();

                let mut download_futures = Vec::new();
                for file in files {
                    let filtered_files = file
                        .product_files
                        .iter()
                        .flat_map(|depot_file| depot_file.get_download_units())
                        .filter(|unit| needs_download_map.contains(&unit.path.as_str()))
                        .collect::<Vec<_>>();
                    if !filtered_files.is_empty() {
                        let stage_tx = stage_tx.clone();
                        let resolver = resolver.clone();
                        let limiter = limiter.clone();
                        let meter = meter.clone();
                        let config = config.clone();
                        let is_dependency = file.is_dependency;
                        download_futures.push(async move {
                            self.download_chunk(
                                filtered_files,
                                resolver,
                                &file.product_id,
                                limiter,
                                meter,
                                config,
                                stage_tx,
                                is_dependency,
                            )
                            .await
                        });
                    }
                }
                // `stage_tx`'s original handle is dropped here at the end of
                // this closure, so the channel closes (and the stage's
                // event-forwarder returns) once every spawned download
                // future below has finished and dropped its own clone.
                futures::future::join_all(download_futures)
            },
        )
        .await;
        controller.abort();

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
        let resolver = Arc::new(PathResolver::new(PathBuf::from(path)).await?);

        let all_files = files
            .iter()
            .flat_map(|f| f.product_files.clone())
            .collect::<Vec<_>>();

        run_stage(
            &tx,
            |event| DownloadJobEvent {
                stage: DownloadStage::Allocating,
                detail: DownloadDetail::Allocation(event),
            },
            |stage_tx| self.allocate_files(all_files, resolver.clone(), stage_tx),
        )
        .await?;

        let (total_bytes, total_chunks) = files
            .iter()
            .flat_map(|f| f.product_files.iter())
            .flat_map(|df| df.get_download_units())
            .fold((0u64, 0usize), |(bytes, chunks), u| (bytes + u.size, chunks + 1));

        // See the comment in `repair_download`: one adaptive concurrency
        // limit shared by every product's chunks in this job, hill-climbed
        // against measured throughput rather than a fixed CPU-derived split.
        let meter = ThroughputMeter::new();
        let limiter = AdaptiveLimiter::new(self.config.min_concurrency);
        let controller = spawn_controller(meter.clone(), limiter.clone(), self.config.clone());
        let config = Arc::new(self.config.clone());

        let results = run_stage(
            &tx,
            |event| DownloadJobEvent {
                stage: DownloadStage::Downloading,
                detail: DownloadDetail::Download(event),
            },
            |stage_tx| {
                stage_tx
                    .send(DownloadEvent::Total {
                        bytes: total_bytes,
                        chunks: total_chunks,
                    })
                    .ok();

                let mut download_futures = Vec::new();
                for file in files {
                    let units = file
                        .product_files
                        .iter()
                        .flat_map(|depot_file| depot_file.get_download_units())
                        .collect::<Vec<_>>();
                    if !units.is_empty() {
                        let stage_tx = stage_tx.clone();
                        let resolver = resolver.clone();
                        let limiter = limiter.clone();
                        let meter = meter.clone();
                        let config = config.clone();
                        let is_dependency = file.is_dependency;
                        download_futures.push(async move {
                            self.download_chunk(
                                units,
                                resolver,
                                &file.product_id,
                                limiter,
                                meter,
                                config,
                                stage_tx,
                                is_dependency,
                            )
                            .await
                        });
                    }
                }
                // See the comment in `repair_download`: `stage_tx`'s
                // original handle drops here, closing the channel once
                // every download future finishes.
                futures::future::join_all(download_futures)
            },
        )
        .await;
        controller.abort();

        for result in results {
            result?;
        }

        Ok(())
    }
    pub async fn verify(
        &self,
        files: Vec<DownloadableFiles>,
        path: &str,
        tx: mpsc::UnboundedSender<VerifyEvent>,
    ) -> Result<(), DownloadError> {
        let resolver = Arc::new(PathResolver::new(PathBuf::from(path)).await?);

        let concurrency = default_concurrency();

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

/// Runs one stage of a multi-stage job: creates a fresh channel, builds the
/// stage's work future via `make_work` (which receives the sending half),
/// and concurrently forwards every event that arrives on it to `out_tx`
/// after tagging it with `wrap`. Returns the work future's result once both
/// the work and the forwarding loop finish.
///
/// Collapses the "channel + forwarder + `tokio::join!`" boilerplate that
/// used to be repeated at every stage boundary in `repair_download` and
/// `download`.
async fn run_stage<E, T, Fut, R>(
    out_tx: &mpsc::UnboundedSender<T>,
    wrap: impl Fn(E) -> T,
    make_work: impl FnOnce(mpsc::UnboundedSender<E>) -> Fut,
) -> R
where
    Fut: Future<Output = R>,
{
    let (stage_tx, mut stage_rx) = mpsc::unbounded_channel::<E>();
    let work = make_work(stage_tx);
    let out_tx = out_tx.clone();
    let receive_future = async move {
        while let Some(event) = stage_rx.recv().await {
            out_tx.send(wrap(event)).ok();
        }
    };
    let (result, ()) = tokio::join!(work, receive_future);
    result
}
