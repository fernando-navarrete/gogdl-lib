//! Tunables for a download job's network behavior: how aggressively chunk
//! concurrency ramps up/down, how long a stalled request is tolerated before
//! it's abandoned, and how many times a failed chunk is retried.
//!
//! Defaults are chosen for network-bound throughput rather than CPU count
//! (see `crate::downloader::adaptive` for why CPU count is the wrong proxy).

use std::time::Duration;

#[derive(Clone, Debug)]
pub struct DownloadConfig {
    /// Floor for the adaptive chunk-concurrency limiter. Never shrinks below
    /// this even under sustained errors, so a job always makes some progress.
    pub min_concurrency: usize,
    /// Ceiling for the adaptive chunk-concurrency limiter.
    pub max_concurrency: usize,
    /// How often the controller re-samples throughput and adjusts
    /// concurrency.
    pub sample_interval: Duration,
    /// Step size (in permits) the controller grows/shrinks concurrency by
    /// each sample.
    pub step: usize,
    /// Deadline for a chunk request to receive its initial response
    /// (headers). Guards against a CDN host that never answers.
    pub response_timeout: Duration,
    /// Deadline for each individual read of a chunk's body stream. Guards
    /// against a connection that answered but then stalled mid-transfer,
    /// which a response-only timeout would never catch.
    pub idle_timeout: Duration,
    /// Maximum number of attempts (primary + redist, combined) per chunk
    /// before it's given up on and reported as a `DownloadError`.
    pub max_retries: usize,
    /// Base delay for exponential backoff between retry attempts; actual
    /// delay is `base_backoff * 2^attempt` plus jitter.
    pub base_backoff: Duration,
}

impl Default for DownloadConfig {
    fn default() -> Self {
        Self {
            min_concurrency: 4,
            max_concurrency: 64,
            sample_interval: Duration::from_millis(750),
            step: 4,
            response_timeout: Duration::from_secs(30),
            idle_timeout: Duration::from_secs(15),
            max_retries: 5,
            base_backoff: Duration::from_millis(200),
        }
    }
}
