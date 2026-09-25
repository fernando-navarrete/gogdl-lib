use std::time::Duration;

pub async fn backoff(attempt: u32) {
    let ceiling = Duration::from_millis(500)
        .saturating_mul(2u32.saturating_pow(attempt))
        .min(Duration::from_secs(20));
    tokio::time::sleep(rand::random_range(Duration::ZERO..=ceiling)).await;
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

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
}
