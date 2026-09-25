/// Something with an optional Unix-seconds deadline that counts as expired
/// [`MARGIN_SECS`](Self::MARGIN_SECS) early, to absorb clock skew and the
/// in-flight request itself.
pub(crate) trait Expiring {
    /// Seconds before the deadline at which it already counts as expired.
    const MARGIN_SECS: i64 = 60;
    /// What [`is_fresh_at`](Self::is_fresh_at) answers when there is no deadline.
    const FRESH_WITHOUT_DEADLINE: bool;

    /// The deadline, in Unix seconds, if known.
    fn deadline(&self) -> Option<i64>;

    /// Whether the deadline is more than [`MARGIN_SECS`](Self::MARGIN_SECS)
    /// after `now` (Unix seconds).
    fn is_fresh_at(&self, now: i64) -> bool {
        match self.deadline() {
            Some(t) => t - Self::MARGIN_SECS > now,
            None => Self::FRESH_WITHOUT_DEADLINE,
        }
    }
}
