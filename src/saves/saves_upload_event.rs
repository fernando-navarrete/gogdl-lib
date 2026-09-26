use std::path::PathBuf;

/// Progress events for
/// [`GogDl::upload_save_files`](crate::GogDl::upload_save_files).
///
/// Reported in stages, one stage per file: [`FileStarted`](Self::FileStarted)
/// opens a file's stage, [`Progress`](Self::Progress) events follow, and
/// [`FileFinished`](Self::FileFinished) closes it. Files are uploaded one at
/// a time, so every `Progress` belongs to the most recent `FileStarted`.
///
/// There is no `ProgressRegression` sibling, unlike
/// [`DownloadEvent`](crate::DownloadEvent): saves are not retried, so no
/// previously-reported byte is ever taken back.
///
/// # Constructing in tests
///
/// Every variant is public, so build them directly. The enum is `#[non_exhaustive]`, so a
/// `match` outside the crate needs a wildcard arm (new variants can arrive in a patch). The type
/// does not implement `Debug` or
/// `PartialEq`; match with `matches!` instead of `assert_eq!`.
///
/// ```
/// use gogdl_lib::SavesUploadEvent;
///
/// let event = SavesUploadEvent::FileFinished { name: "slot".into() };
/// assert!(matches!(event, SavesUploadEvent::FileFinished { .. }));
/// ```
#[non_exhaustive]
pub enum SavesUploadEvent {
    /// Emitted once, before the first file. `total_files` is `0` if the
    /// directory holds no files.
    Preparing {
        /// How many files will be uploaded.
        total_files: usize,
    },
    /// A file's stage has begun.
    FileStarted {
        /// The file's name in cloud storage once uploaded, as in
        /// [`SaveFile::name`](crate::SaveFile::name).
        name: String,
        /// The local file being uploaded.
        source: PathBuf,
        /// The number of bytes that will be sent for this file — its
        /// gzip-compressed size, and the denominator for the `Progress`
        /// deltas that follow.
        total_bytes: u64,
    },
    /// Bytes handed to the network since the last `Progress` event for the
    /// current file — a delta, not a running total. Reported as they are
    /// queued for sending, not as GOG acknowledges them.
    Progress(usize),
    /// The current file was accepted by GOG.
    FileFinished {
        /// The file's name in cloud storage.
        name: String,
    },
}
