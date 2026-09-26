//! The one place that decides how often and how patiently a failed request or chunk is retried.
//!
//! **Policy.** [`backoff`] sleeps a uniformly random time in `[0, ceiling]` (full jitter), where
//! `ceiling = 500ms * 2^attempt`, capped at 20s.
//!
//! **Budgets.** Transport, HTTP and short-write failures get [`MAX_ATTEMPTS`] tries. A chunk whose
//! MD5 doesn't match gets [`MAX_HASH_ATTEMPTS`]: a corrupt body is rarely fixed by asking again, so
//! it isn't worth the full budget.
//!
//! **De-facto budget against a dead CDN.** Six attempts means five backoffs, at attempts 0 to 4,
//! with ceilings of 0.5 + 1 + 2 + 4 + 8 = 15.5s. A chunk therefore gives up after 15.5s at worst
//! and about 7.75s on average, plus the time each attempt takes to fail. The 20s cap never binds,
//! because the largest `attempt` [`backoff`] sees is 4; it only guards a larger `MAX_ATTEMPTS`.

use std::time::Duration;

/// How many times a request or chunk is tried before its error is returned.
pub(crate) const MAX_ATTEMPTS: u32 = 6;

/// How many times a chunk is tried when its decompressed MD5 doesn't match.
pub(crate) const MAX_HASH_ATTEMPTS: u32 = 3;

/// Sleeps for a random time up to this attempt's ceiling (see the module docs).
pub(crate) async fn backoff(attempt: u32) {
    let ceiling = Duration::from_millis(500)
        .saturating_mul(2u32.saturating_pow(attempt))
        .min(Duration::from_secs(20));
    tokio::time::sleep(rand::random_range(Duration::ZERO..=ceiling)).await;
}

/// Whether a failed `attempt` (counting from 0) is followed by another one under `bound`.
fn retries_left(attempt: u32, bound: u32) -> bool {
    attempt + 1 < bound
}

/// If `attempt` isn't the last one under `bound`, backs off and returns `Ok(())` so the caller can
/// `continue`; otherwise returns `err`.
pub(crate) async fn retry_or_return<E>(attempt: u32, bound: u32, err: E) -> Result<(), E> {
    if retries_left(attempt, bound) {
        backoff(attempt).await;
        Ok(())
    } else {
        Err(err)
    }
}

/// [`retry_or_return`] without the sleep, for a failure the retry itself repairs (a 401 makes the
/// caller fetch fresh links, so there's nothing to wait for).
pub(crate) fn retry_now_or_return<E>(attempt: u32, bound: u32, err: E) -> Result<(), E> {
    if retries_left(attempt, bound) {
        Ok(())
    } else {
        Err(err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn sleeps_at_most_the_ceiling_for_the_attempt() {
        let table = [
            (0, Duration::from_millis(500)),
            (1, Duration::from_secs(1)),
            (2, Duration::from_secs(2)),
            (3, Duration::from_secs(4)),
            (4, Duration::from_secs(8)),
            (5, Duration::from_secs(16)),
            // 32s would be next; it's capped at 20s.
            (6, Duration::from_secs(20)),
            (10, Duration::from_secs(20)),
            // 2^attempt overflows u32; the saturating ops keep the cap.
            (u32::MAX, Duration::from_secs(20)),
        ];
        for (attempt, ceiling) in table {
            let mut longest = Duration::ZERO;
            for _ in 0..200 {
                let start = tokio::time::Instant::now();
                backoff(attempt).await;
                longest = longest.max(start.elapsed());
            }
            assert!(
                longest <= ceiling,
                "attempt {attempt}: slept {longest:?}, ceiling {ceiling:?}"
            );
            // Jitter spans the range rather than sitting at zero: the
            // longest of 200 draws lands in the top half of the ceiling.
            assert!(
                longest > ceiling / 2,
                "attempt {attempt}: longest of 200 sleeps was only {longest:?}"
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn retry_or_return_backs_off_until_the_last_attempt() {
        for attempt in 0..MAX_HASH_ATTEMPTS - 1 {
            assert_eq!(
                retry_or_return(attempt, MAX_HASH_ATTEMPTS, "e").await,
                Ok(())
            );
        }
        assert_eq!(
            retry_or_return(MAX_HASH_ATTEMPTS - 1, MAX_HASH_ATTEMPTS, "e").await,
            Err("e")
        );
    }

    #[tokio::test(start_paused = true)]
    async fn retry_now_or_return_never_sleeps() {
        let start = tokio::time::Instant::now();
        for attempt in 0..MAX_ATTEMPTS - 1 {
            assert_eq!(retry_now_or_return(attempt, MAX_ATTEMPTS, "e"), Ok(()));
        }
        assert_eq!(
            retry_now_or_return(MAX_ATTEMPTS - 1, MAX_ATTEMPTS, "e"),
            Err("e")
        );
        assert_eq!(start.elapsed(), Duration::ZERO);
    }
}
