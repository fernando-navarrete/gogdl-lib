//! The `Downloader` engine: orchestrates the multi-stage `download`,
//! `repair_download`, and `verify` jobs by driving the per-stage helpers in
//! `stages.rs` and forwarding their events, tagged by stage, to the
//! caller's channel.

use std::{collections::HashSet, future::Future, path::PathBuf, sync::Arc};

use crate::{
    DownloadableFiles,
    auth::AuthManager,
    client::HttpClient,
    depot::DownloadUnit,
    downloader::{
        DownloadConfig, DownloadError,
        adaptive::{AdaptiveLimiter, ThroughputMeter, spawn_controller},
        events::{
            DownloadDetail, DownloadEvent, DownloadJobEvent, DownloadStage, RepairDetail,
            RepairEvent, RepairStage, VerifyEvent,
        },
        stages::{ChunkVerifyIssue, DownloadStageCtx, check_unit_checksum, default_concurrency},
        util::PathResolver,
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

        self.run_download_stage(
            files,
            resolver,
            |unit| needs_download_map.contains(&unit.path.as_str()),
            &tx,
            |event| RepairEvent {
                stage: RepairStage::Downloading,
                detail: RepairDetail::Download(event),
            },
        )
        .await
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

        self.run_download_stage(
            files,
            resolver,
            |_| true,
            &tx,
            |event| DownloadJobEvent {
                stage: DownloadStage::Downloading,
                detail: DownloadDetail::Download(event),
            },
        )
        .await
    }
    /// The download stage shared by `download` and `repair_download`: totals
    /// up the units selected by `should_download`, spins up the job's
    /// adaptive concurrency controller, and downloads every selected unit of
    /// every product, forwarding `DownloadEvent`s to `out_tx` via `wrap`.
    async fn run_download_stage<T>(
        &self,
        files: Vec<DownloadableFiles>,
        resolver: Arc<PathResolver>,
        should_download: impl Fn(&DownloadUnit) -> bool,
        out_tx: &mpsc::UnboundedSender<T>,
        wrap: impl Fn(DownloadEvent) -> T,
    ) -> Result<(), DownloadError> {
        let (total_bytes, total_chunks) = files
            .iter()
            .flat_map(|f| f.product_files.iter())
            .flat_map(|df| df.get_download_units())
            .filter(|u| should_download(u))
            .fold((0u64, 0usize), |(bytes, chunks), u| {
                (bytes + u.size, chunks + 1)
            });

        // One adaptive concurrency limit shared by every product's chunks in
        // this job — not a fixed, CPU-derived number per product (see
        // `downloader::adaptive`). The controller hill-climbs it against
        // measured throughput for the lifetime of the download stage; its
        // sampler loop runs forever, so it is aborted below as soon as the
        // stage returns.
        let meter = ThroughputMeter::new();
        let limiter = AdaptiveLimiter::new(self.config.min_concurrency);
        let controller = spawn_controller(meter.clone(), limiter.clone(), self.config.clone());

        let results = run_stage(out_tx, wrap, |stage_tx| {
            stage_tx
                .send(DownloadEvent::Total {
                    bytes: total_bytes,
                    chunks: total_chunks,
                })
                .ok();

            let ctx = DownloadStageCtx {
                client: self.client.clone(),
                resolver,
                limiter,
                meter,
                config: Arc::new(self.config.clone()),
                tx: stage_tx,
            };

            let mut download_futures = Vec::new();
            for file in files {
                let units = file
                    .product_files
                    .iter()
                    .flat_map(|depot_file| depot_file.get_download_units())
                    .filter(|unit| should_download(unit))
                    .collect::<Vec<_>>();
                if !units.is_empty() {
                    let ctx = ctx.clone();
                    download_futures.push(async move {
                        self.download_chunk(units, &file.product_id, ctx, file.is_dependency)
                            .await
                    });
                }
            }
            // `ctx` (and with it the original `stage_tx` handle) is dropped
            // here at the end of this closure, so the channel closes (and
            // the stage's event-forwarder returns) once every download
            // future below has finished and dropped its own clone.
            futures::future::join_all(download_futures)
        })
        .await;

        // Aborted before the `?` below rather than after, so an errored unit
        // can't return early and leave the sampler task spinning forever.
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

        let chunks = files
            .iter()
            .flat_map(|files| files.product_files.iter())
            .flat_map(|file| file.get_download_units())
            .collect::<Vec<_>>();

        stream::iter(chunks)
            .map(|chunk| {
                let resolver = resolver.clone();
                let tx = tx.clone();
                async move {
                    match check_unit_checksum(&resolver, &chunk).await {
                        Ok(()) => {
                            tx.send(VerifyEvent::ChunkOk).ok();
                        }
                        Err(ChunkVerifyIssue::PathResolveError(e)) => {
                            tx.send(VerifyEvent::CouldNotResolvePath(chunk.path.clone()))
                                .ok();
                            println!("ERROR: {}: {}", chunk.path.clone(), e);
                        }
                        Err(ChunkVerifyIssue::FileNotFound) => {
                            tx.send(VerifyEvent::FileNotFound(chunk.path.clone())).ok();
                            println!("ERROR: {}: FILE NOT FOUND", chunk.path.clone());
                        }
                        Err(ChunkVerifyIssue::ChecksumCalculationError(final_path, e)) => {
                            tx.send(VerifyEvent::ChunkChecksumMismatch(
                                final_path.to_string_lossy().to_string(),
                            ))
                            .ok();
                            println!("ERROR: {}: {}", chunk.path.clone(), e);
                        }
                        Err(ChunkVerifyIssue::ChecksumMismatch(final_path)) => {
                            tx.send(VerifyEvent::ChunkChecksumMismatch(
                                final_path.to_string_lossy().to_string(),
                            ))
                            .ok();
                            println!("ERROR: {} CHECKSUM MISMATCH", chunk.path.clone());
                        }
                    }
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
