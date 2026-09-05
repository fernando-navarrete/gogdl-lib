/// Progress events for the download stage — the part of
/// [`GogDl::download_game`](crate::GogDl::download_game)/
/// [`GogDl::repair_game`](crate::GogDl::repair_game) that actually transfers
/// chunk bytes, wrapped in
/// [`DownloadStageEvent::DownloadStage`](crate::DownloadStageEvent::DownloadStage).
pub enum DownloadEvent {
    /// Emitted once, immediately before the transfer stage begins.
    Preparing,
    /// Emitted once, immediately after `Preparing` with no work in between —
    /// a stage marker, not evidence that any preparation actually happened.
    Prepared,
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
    /// size, and there is currently no exposed compressed total to divide
    /// by for a percentage.
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
    ProgressRegression(usize),
}
