/// One chunk's result during a checksum-verification pass — used by
/// [`GogDl::verify_files`](crate::GogDl::verify_files) directly, and by
/// [`GogDl::repair_game`](crate::GogDl::repair_game)'s
/// [`DownloadStageEvent::VerificationStage`](crate::DownloadStageEvent::VerificationStage).
/// Every payload is `(relative path, uncompressed chunk size)`. Anything
/// other than `Verified` means the chunk needs downloading.
///
/// # Constructing in tests
///
/// Every variant is public, so build them directly. The type does not implement
/// `Debug` or `PartialEq`; match with `matches!` instead of `assert_eq!`.
///
/// ```
/// use gogdl_lib::VerificationEvent;
///
/// let event = VerificationEvent::ChecksumMismatch("game.exe".into(), 10);
/// assert!(matches!(event, VerificationEvent::ChecksumMismatch(_, 10)));
/// ```
pub enum VerificationEvent {
    /// The file's relative path could not be resolved to a location on
    /// disk.
    CouldNotResolvePath(String, u64),
    /// No file exists yet at the resolved path.
    FileNotFound(String, u64),
    /// The chunk's on-disk MD5 didn't match the manifest (or the file was
    /// too short to read the chunk's declared range).
    ChecksumMismatch(String, u64),
    /// The chunk's on-disk content matches the manifest.
    Verified(String, u64),
}
