use std::path::PathBuf;

/// Progress events for
/// [`GogDl::download_save_files`](crate::GogDl::download_save_files).
///
/// Reported in stages, one stage per file: [`FileStarted`](Self::FileStarted)
/// opens a file's stage, [`Progress`](Self::Progress) events follow, and
/// [`FileFinished`](Self::FileFinished) closes it. Files are downloaded one
/// at a time, so every `Progress` belongs to the most recent `FileStarted`.
///
/// There is no `ProgressRegression` sibling, unlike
/// [`DownloadEvent`](crate::DownloadEvent): saves are not retried, so no
/// previously-reported byte is ever taken back.
///
/// # Constructing in tests
///
/// Every variant is public, so build them directly. The type does not implement
/// `Debug` or `PartialEq`; match with `matches!` instead of `assert_eq!`.
///
/// ```
/// use gogdl_lib::SavesDownloadEvent;
///
/// let event = SavesDownloadEvent::Preparing { total_files: 1, total_bytes: 9 };
/// assert!(matches!(event, SavesDownloadEvent::Preparing { total_files: 1, .. }));
/// ```
pub enum SavesDownloadEvent {
    /// Emitted once, before the first file. Describes the whole job; both
    /// fields are `0` when the game has no cloud saves.
    Preparing {
        /// How many files will be downloaded.
        total_files: usize,
        /// The combined size of those files as stored in the cloud, in bytes.
        total_bytes: u64,
    },
    /// A file's stage has begun.
    FileStarted {
        /// The file's name in cloud storage, as in
        /// [`SaveFile::name`](crate::SaveFile::name).
        name: String,
        /// Where the file will be written: inside the directory its save
        /// location expands to in the Wine prefix passed to
        /// [`GogDl::download_save_files`](crate::GogDl::download_save_files).
        destination: PathBuf,
        /// The file's size as stored in the cloud, in bytes — the
        /// denominator for the `Progress` deltas that follow.
        total_bytes: u64,
    },
    /// Bytes received off the wire since the last `Progress` event for the
    /// current file — a delta, not a running total. Counts stored
    /// (compressed) bytes, so summing a file's deltas converges to its
    /// `total_bytes`.
    Progress(usize),
    /// The current file is fully written, its hash verified, and its
    /// modification time restored.
    FileFinished {
        /// The file's name in cloud storage.
        name: String,
    },
}
