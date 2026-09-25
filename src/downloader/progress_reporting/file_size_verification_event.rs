/// One file's result during the initial size-verification stage
/// (wrapped in
/// [`DownloadStageEvent::FileSizeVerificationStage`](crate::DownloadStageEvent::FileSizeVerificationStage)),
/// run before allocation on every
/// [`GogDl::download_game`](crate::GogDl::download_game)/
/// [`GogDl::repair_game`](crate::GogDl::repair_game) call. Every payload is
/// `(relative path, expected uncompressed size)`. Any variant other than
/// `FileSizeVerificationSuccess` means the file goes on to the allocation
/// stage.
///
/// # Constructing in tests
///
/// Every variant is public, so build them directly. The type does not implement
/// `Debug` or `PartialEq`; match with `matches!` instead of `assert_eq!`.
///
/// ```
/// use gogdl_lib::FileSizeVerificationEvent;
///
/// let event = FileSizeVerificationEvent::FileNotFound("game.exe".into(), 10);
/// assert!(matches!(event, FileSizeVerificationEvent::FileNotFound(_, 10)));
/// ```
pub enum FileSizeVerificationEvent {
    /// The manifest lists this file with no chunks; it was skipped. Size is
    /// always `0`.
    FileWithNoChunks(String, u64),
    /// The file's relative path could not be resolved to a location on
    /// disk.
    CouldNotResolvePath(String, u64),
    /// No file exists yet at the resolved path.
    FileNotFound(String, u64),
    /// The file exists, but its metadata (size) could not be read.
    FileSizeVerificationFailed(String, u64),
    /// The file exists but its on-disk size doesn't match the expected
    /// size.
    FileSizeMismatch(String, u64),
    /// The file exists and already has the expected size. Note this checks
    /// size only, not content — a same-size but corrupt file passes this
    /// stage and is only caught by `repair_game`'s later checksum pass, not
    /// by `download_game`.
    FileSizeVerificationSuccess(String, u64),
}
