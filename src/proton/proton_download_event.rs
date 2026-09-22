/// Progress events for
/// [`GogDl::download_proton_release`](crate::GogDl::download_proton_release).
///
/// Unlike [`DownloadEvent`](crate::DownloadEvent), there is no
/// `ProgressRegression` sibling here: this download has no retry loop (a
/// failed transfer fails the whole call), so no previously-reported byte is
/// ever taken back. `Progress` and `Downloading`'s `total_bytes` are also
/// the same unit (compressed wire bytes), so summing every `Progress`
/// payload does converge to `total_bytes` — unlike `DownloadEvent::Progress`,
/// whose compressed/uncompressed unit mismatch against its sizing events is
/// a known rough edge.
pub enum ProtonDownloadEvent {
    /// Emitted once, before the first byte is read. `total_bytes` is the
    /// release asset's compressed size, as reported by GitHub — the
    /// denominator for the `Progress` deltas that follow.
    Downloading {
        /// The compressed tarball size, in bytes.
        total_bytes: u64,
    },
    /// Compressed bytes read off the wire since the last `Progress` event —
    /// a delta, not a running total. The tarball is decompressed and
    /// extracted as it streams in, so this does not correspond to any
    /// on-disk byte count.
    ///
    /// Emitted once per network read, by design — same caller-owned
    /// coalescing contract as [`DownloadEvent::Progress`](crate::DownloadEvent::Progress).
    /// This is a single stream rather than several chunks in flight, so the
    /// rate is lower, but it is still uncoalesced: accumulate and flush on
    /// your own interval before handing deltas to a UI sink.
    Progress(usize),
    /// One entry (file, directory or symlink) has been extracted to disk.
    /// The payload is that entry's path, relative to the destination
    /// directory passed to
    /// [`GogDl::download_proton_release`](crate::GogDl::download_proton_release).
    Extracted(String),
}
