//! Adaptive chunk-concurrency control.
//!
//! The old concurrency model derived a fixed chunk-download parallelism from
//! `std::thread::available_parallelism()` (logical CPU count). That's a
//! reasonable proxy for CPU-bound work, but chunk downloads are network-bound
//! — the right amount of parallelism depends on the link's bandwidth-delay
//! product and the CDN's per-connection throttling, neither of which has
//! anything to do with how many cores the machine has. A 4-core machine on a
//! gigabit line and an 8-core machine on the same line both want roughly the
//! same (large) number of concurrent connections; CPU count predicts neither.
//!
//! This module replaces that fixed limit with one shared, job-global
//! `AdaptiveLimiter` whose permit count a background controller hill-climbs
//! against *measured* throughput (`ThroughputMeter`), backing off when
//! transient errors spike — which is the actual signal that concurrency has
//! gone past what the CDN/network will tolerate.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::sync::{Semaphore, SemaphorePermit};
use tokio::task::JoinHandle;

use crate::downloader::config::DownloadConfig;
use crate::downloader::control::DownloadControl;

/// Cumulative byte/transient-error counters fed by every in-flight chunk.
/// The controller samples these once per interval and derives deltas itself;
/// counters here are monotonically increasing and never reset.
#[derive(Default)]
pub(crate) struct ThroughputMeter {
    bytes: AtomicU64,
    transient_errors: AtomicU64,
}

impl ThroughputMeter {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub(crate) fn add_bytes(&self, n: u64) {
        self.bytes.fetch_add(n, Ordering::Relaxed);
    }

    pub(crate) fn add_transient_error(&self) {
        self.transient_errors.fetch_add(1, Ordering::Relaxed);
    }

    fn snapshot(&self) -> (u64, u64) {
        (
            self.bytes.load(Ordering::Relaxed),
            self.transient_errors.load(Ordering::Relaxed),
        )
    }
}

/// A semaphore-backed concurrency limit that a controller task can grow or
/// shrink at runtime. `tokio::sync::Semaphore` has no `remove_permits`, so
/// shrinking works by permanently acquiring-and-forgetting permits: once
/// enough in-flight permits are returned to satisfy the acquisition, they
/// simply never re-enter the pool.
pub(crate) struct AdaptiveLimiter {
    semaphore: Semaphore,
}

impl AdaptiveLimiter {
    pub(crate) fn new(initial: usize) -> Arc<Self> {
        Arc::new(Self {
            semaphore: Semaphore::new(initial.max(1)),
        })
    }

    /// Acquires one permit, parking until either a permit is free or the
    /// controller grows the limit. The semaphore is never closed, so this
    /// can't fail.
    pub(crate) async fn acquire(&self) -> SemaphorePermit<'_> {
        self.semaphore
            .acquire()
            .await
            .expect("adaptive limiter semaphore is never closed")
    }

    /// The concurrency limit currently in effect. Only meaningful as "the
    /// limit" when nothing is concurrently holding a permit (e.g. right
    /// after construction, or in a test driving the controller directly) —
    /// once real work is in flight this returns permits merely *available*
    /// right now, not the total limit. `spawn_controller` uses this once,
    /// at startup, to learn its actual starting point rather than assuming
    /// one.
    pub(crate) fn current_limit(&self) -> usize {
        self.semaphore.available_permits()
    }

    fn grow(&self, by: usize) {
        if by > 0 {
            self.semaphore.add_permits(by);
        }
    }

    /// Shrinks the limit by `by`, waiting for that many permits to become
    /// available from in-flight work before removing them. Runs from the
    /// controller's own background task, so this delay never blocks a chunk
    /// download.
    async fn shrink(&self, by: usize) {
        if by == 0 {
            return;
        }
        if let Ok(permits) = self.semaphore.acquire_many(by as u32).await {
            permits.forget();
        }
    }
}

/// Ignore throughput deltas smaller than this fraction of the previous
/// sample — below this, the change is measurement jitter, not signal.
const NOISE_FLOOR: f64 = 0.05;
/// Number of transient errors within one sample window that's treated as a
/// concurrency-driven spike (CDN rate-limiting, connection resets under
/// load) rather than ordinary background flakiness.
const ERROR_SPIKE_THRESHOLD: u64 = 3;

/// Handle to the spawned controller task; aborts it on drop. The sampler
/// loop otherwise runs forever, so tying its lifetime to this handle (held
/// across the download stage) guarantees it can't leak — even when the
/// job's future is dropped mid-stage instead of running to completion,
/// which a plain `JoinHandle` + explicit `abort()` call would miss.
pub(crate) struct ControllerHandle(JoinHandle<()>);

impl Drop for ControllerHandle {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Spawns the background task that hill-climbs chunk concurrency against
/// measured throughput once per `config.sample_interval`, backing off when
/// transient errors spike. The loop runs until the returned
/// `ControllerHandle` is dropped.
///
/// `control` is consulted each sample: while the job is paused, no chunk
/// bytes are flowing, so throughput reads as ~0 and would otherwise look
/// like "concurrency is too high," shrinking the limit toward the floor for
/// no reason and making resume ramp back up slowly. Instead, while paused,
/// the sample is skipped and the bytes/error baseline is refreshed so the
/// first post-resume sample measures a real window instead of a starved one.
pub(crate) fn spawn_controller(
    meter: Arc<ThroughputMeter>,
    limiter: Arc<AdaptiveLimiter>,
    config: DownloadConfig,
    control: DownloadControl,
) -> ControllerHandle {
    ControllerHandle(tokio::spawn(async move {
        // Read the limiter's actual starting permit count rather than
        // assuming `config.min_concurrency`: the two always match in
        // production (the limiter is always constructed with exactly that
        // many permits right before this is spawned), but tying this to
        // `config` instead of the `limiter` it's actually controlling is an
        // avoidable, fragile coupling.
        let mut current = limiter.current_limit().max(1);
        let mut direction: i64 = 1; // +1 = probing more concurrency, -1 = backing off
        let mut last_throughput: f64 = 0.0;
        let mut last_bytes = 0u64;
        let mut last_errors = 0u64;

        loop {
            tokio::time::sleep(config.sample_interval).await;

            let (bytes, errors) = meter.snapshot();

            if control.is_paused() {
                // Re-baseline without adjusting concurrency: see the doc
                // comment above.
                last_bytes = bytes;
                last_errors = errors;
                last_throughput = 0.0;
                continue;
            }

            let bytes_delta = bytes.saturating_sub(last_bytes);
            let errors_delta = errors.saturating_sub(last_errors);
            last_bytes = bytes;
            last_errors = errors;

            let throughput = bytes_delta as f64 / config.sample_interval.as_secs_f64();

            // A burst of transient errors is a stronger, more direct signal
            // than the throughput curve: retreat immediately regardless of
            // what the hill-climb would otherwise do.
            if errors_delta >= ERROR_SPIKE_THRESHOLD {
                let shrink_by = (current / 2).max(config.step);
                let target = current
                    .saturating_sub(shrink_by)
                    .max(config.min_concurrency);
                if target < current {
                    limiter.shrink(current - target).await;
                    current = target;
                }
                direction = -1;
                last_throughput = throughput;
                continue;
            }

            // Hill-climb: keep moving in `direction` if throughput improved
            // meaningfully since the last sample; reverse if it regressed or
            // was flat. This finds the knee of the throughput-vs-concurrency
            // curve (where the CDN/link stops giving back more speed for
            // more connections) without ever hardcoding what that knee is.
            if last_throughput > 0.0 {
                let change = (throughput - last_throughput) / last_throughput;
                if change < NOISE_FLOOR {
                    direction = -direction;
                }
            }

            let next = if direction > 0 {
                (current + config.step).min(config.max_concurrency)
            } else {
                current
                    .saturating_sub(config.step)
                    .max(config.min_concurrency)
            };

            if next > current {
                limiter.grow(next - current);
            } else if next < current {
                limiter.shrink(current - next).await;
            }
            current = next;
            last_throughput = throughput;
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn test_config(min: usize, max: usize, step: usize, sample_interval_ms: u64) -> DownloadConfig {
        DownloadConfig {
            min_concurrency: min,
            max_concurrency: max,
            sample_interval: Duration::from_millis(sample_interval_ms),
            step,
            ..DownloadConfig::default()
        }
    }

    /// Simulates a link whose throughput rises linearly with concurrency up
    /// to `knee` connections, then plateaus — modeling a CDN's
    /// per-connection throttle: more connections help until you hit it,
    /// then buy nothing.
    fn knee_throughput(concurrency: usize, knee: usize, per_connection: u64) -> u64 {
        concurrency.min(knee) as u64 * per_connection
    }

    /// Drives the meter/limiter feedback loop for one controller sample
    /// window using tokio's paused virtual clock instead of real wall-clock
    /// sleeps: real sleeps of a few milliseconds are exactly the kind of
    /// timing that's unreliable under OS scheduling jitter (a growth this
    /// test just applied might not be "seen" until a sample or two later
    /// purely from real-time drift, making the test flaky rather than the
    /// algorithm being wrong). `tokio::time::advance` instead jumps the
    /// clock forward by exactly one interval and lets the controller's
    /// pending `sleep` resolve deterministically, so the same concurrency
    /// value is in effect for the whole window that's about to be measured.
    async fn simulate_one_window(
        meter: &ThroughputMeter,
        limiter: &AdaptiveLimiter,
        interval: Duration,
        throughput_for: impl Fn(usize) -> u64,
    ) {
        let concurrency = limiter.current_limit();
        let bytes = throughput_for(concurrency) * interval.as_millis() as u64 / 1000;
        meter.add_bytes(bytes);
        tokio::time::advance(interval).await;
    }

    #[tokio::test(start_paused = true)]
    async fn hill_climbs_toward_the_throughput_knee() {
        let knee = 20;
        let per_connection = 1_000_000u64; // 1 MB/s per connection below the knee

        let config = test_config(2, 200, 2, 100);
        let meter = ThroughputMeter::new();
        let limiter = AdaptiveLimiter::new(config.min_concurrency);
        let controller = spawn_controller(
            meter.clone(),
            limiter.clone(),
            config.clone(),
            DownloadControl::new(),
        );

        for _ in 0..80 {
            simulate_one_window(&meter, &limiter, config.sample_interval, |c| {
                knee_throughput(c, knee, per_connection)
            })
            .await;
        }
        drop(controller);

        let final_limit = limiter.current_limit();
        // The hill-climb should have found its way to (and then oscillate
        // around) the knee, not stayed at the floor or run to the ceiling.
        assert!(
            final_limit > config.min_concurrency * 2,
            "expected concurrency to grow well past the floor, got {final_limit}"
        );
        assert!(
            final_limit < knee * 3,
            "expected concurrency to settle near the knee ({knee}), got {final_limit}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn backs_off_on_transient_error_spike() {
        let config = test_config(2, 200, 2, 100);
        let meter = ThroughputMeter::new();
        let limiter = AdaptiveLimiter::new(20);
        let controller = spawn_controller(
            meter.clone(),
            limiter.clone(),
            config.clone(),
            DownloadControl::new(),
        );

        // Let the freshly-spawned controller task run once so it reaches
        // its first `sleep` and actually registers a timer — otherwise the
        // single `advance` below would just prime that first sleep instead
        // of firing it, and the controller would never see the errors added
        // after it.
        tokio::task::yield_now().await;

        // A burst of transient errors: the controller should treat this as
        // a signal to back off immediately, regardless of the throughput
        // trend — the error-spike check runs before the hill-climb
        // comparison.
        meter.add_transient_error();
        meter.add_transient_error();
        meter.add_transient_error();

        tokio::time::advance(config.sample_interval).await;
        // `advance` fires the controller's due timer but, on a
        // current-thread runtime, doesn't itself guarantee the now-woken
        // controller task has been polled to completion before returning —
        // give the scheduler an explicit turn to run it.
        tokio::task::yield_now().await;
        drop(controller);

        let after = limiter.current_limit();
        assert!(
            after < 20,
            "expected concurrency to shrink after an error spike: after={after}"
        );
    }
}
