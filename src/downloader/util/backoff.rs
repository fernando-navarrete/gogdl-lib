use std::time::Duration;

pub async fn backoff(attempt: u32) {
    let ceiling = Duration::from_millis(500)
        .saturating_mul(2u32.saturating_pow(attempt))
        .min(Duration::from_secs(20));
    tokio::time::sleep(rand::random_range(Duration::ZERO..=ceiling)).await;
}
