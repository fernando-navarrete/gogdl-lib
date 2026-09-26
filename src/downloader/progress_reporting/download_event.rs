/// Progress events for the download stage — the part of
/// [`GogDl::download_game`](crate::GogDl::download_game)/
/// [`GogDl::repair_game`](crate::GogDl::repair_game) that actually transfers
/// chunk bytes, wrapped in
/// [`DownloadStageEvent::DownloadStage`](crate::DownloadStageEvent::DownloadStage).
///
/// # Constructing in tests
///
/// Every variant is public, so build them directly. The enum is `#[non_exhaustive]`, so a
/// `match` outside the crate needs a wildcard arm (new variants can arrive in a patch). The type
/// does not implement `Debug` or
/// `PartialEq`; match with `matches!` instead of `assert_eq!`.
///
/// ```
/// use gogdl_lib::DownloadEvent;
///
/// let event = DownloadEvent::Progress(4096);
/// assert!(matches!(event, DownloadEvent::Progress(4096)));
/// ```
#[non_exhaustive]
pub enum DownloadEvent {
    /// Emitted once, immediately before the transfer stage begins.
    ///
    /// Deprecated: it is still sent, back-to-back with `Prepared` and just
    /// before [`Started`](Self::Started), which carries the same "the transfer
    /// stage begins" signal plus the compressed total. Removed in `v1.4.0`.
    #[deprecated(
        since = "1.2.3",
        note = "use `DownloadEvent::Started`; removed in 1.4.0"
    )]
    Preparing,
    /// Emitted once, immediately after `Preparing` with no work in between —
    /// a stage marker, not evidence that any preparation actually happened.
    ///
    /// Deprecated: still sent, just before [`Started`](Self::Started). Removed
    /// in `v1.4.0`.
    #[deprecated(
        since = "1.2.3",
        note = "use `DownloadEvent::Started`; removed in 1.4.0"
    )]
    Prepared,
    /// Emitted once per transfer stage, after `Prepared` and before the first
    /// `Downloading`.
    ///
    /// `compressed_total` is the sum of the manifest's compressed chunk sizes
    /// over the units this stage will transfer: every chunk for
    /// [`GogDl::download_game`](crate::GogDl::download_game), and only the
    /// chunks that failed verification for
    /// [`GogDl::repair_game`](crate::GogDl::repair_game). It is the
    /// denominator for [`Progress`](Self::Progress): once the stage succeeds,
    /// the `Progress` deltas minus the
    /// [`ProgressRegression`](Self::ProgressRegression) deltas add up to it.
    /// A retried chunk doesn't change that, since each failed attempt is taken
    /// back.
    Started {
        /// Compressed bytes the stage will transfer.
        compressed_total: u64,
    },
    /// Emitted once per download unit (chunk), at the moment it is
    /// scheduled — not when it completes. With several units in flight
    /// concurrently, several `Downloading` events can precede the first
    /// `Progress`.
    Downloading,
    /// Bytes received for the current chunk's attempt, as raw compressed
    /// bytes off the wire — **not** the uncompressed size any
    /// [`FileSizeVerificationEvent`](crate::FileSizeVerificationEvent)/
    /// [`VerificationEvent`](crate::VerificationEvent) reports. Summing
    /// every `Progress` payload will not converge to the install's on-disk
    /// size; divide the net sum (`Progress` minus `ProgressRegression`) by
    /// [`Started::compressed_total`](Self::Started) for a percentage.
    ///
    /// **Emitted once per network read, by design.** This crate reports at
    /// the finest granularity it has — one event per `Bytes` chunk yielded
    /// by the underlying stream — and deliberately does not coalesce or
    /// throttle them itself. Coalescing is the caller's job: it is the only
    /// place that knows what display rate a frontend actually wants (a
    /// terminal progress bar, a UI meter animated at 60fps, a log line every
    /// few seconds). Forwarding these 1:1 into an unbounded channel that
    /// feeds something slower than the network (e.g. a UI thread, a bridge
    /// to another runtime) will flood it; accumulate the deltas into a
    /// counter and flush on your own interval (tens of milliseconds is
    /// usually enough) before handing them to a UI sink. This crate will not
    /// add rate-limiting on the consumer's behalf, since that would fix one
    /// display rate for every caller.
    Progress(usize),
    /// A previously-reported `Progress` total for the current chunk must be
    /// taken back, because the attempt that reported it failed. The payload
    /// is a **delta to subtract** from your running total, not a
    /// replacement value — and it can exceed what you've accumulated for
    /// this chunk if it arrives after several successful reads followed by
    /// a late failure, so fold it with `saturating_sub`, not plain `-=`.
    ///
    /// Emitted before both a retry (`continue`) *and* a terminal failure
    /// (`return Err`) — receiving this does **not** imply another `Progress`
    /// for the same chunk will follow. Whether it does depends on whether
    /// the failure was on the chunk's last allowed attempt.
    ///
    /// Also emitted when a unit is cancelled mid-transfer because another
    /// unit in the same batch failed terminally (see
    /// [`DownloadError::ChunkHashMismatch`](crate::DownloadError::ChunkHashMismatch)
    /// and the batch-abort note on
    /// [`GogDl::download_game`](crate::GogDl::download_game)) — cancellation
    /// takes back whatever that unit had reported, the same as any other
    /// incomplete attempt. The invariant holds unconditionally: every
    /// attempt that reported at least one `Progress` byte and did not reach
    /// a successful chunk emits exactly one matching `ProgressRegression`,
    /// so folding with `saturating_sub` always converges to zero for that
    /// chunk.
    ProgressRegression(usize),
}
