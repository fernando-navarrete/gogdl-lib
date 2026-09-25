/// One file's result during the file-allocation stage
/// (wrapped in
/// [`DownloadStageEvent::FileAllocationStage`](crate::DownloadStageEvent::FileAllocationStage)).
/// Every payload is `(relative path, expected uncompressed size)`.
///
/// # Constructing in tests
///
/// Every variant is public, so build them directly. The type does not implement
/// `Debug` or `PartialEq`; match with `matches!` instead of `assert_eq!`.
///
/// ```
/// use gogdl_lib::FileAllocationEvent;
///
/// let event = FileAllocationEvent::FileAllocationSuccess("game.exe".into(), 10);
/// assert!(matches!(event, FileAllocationEvent::FileAllocationSuccess(_, 10)));
/// ```
pub enum FileAllocationEvent {
    /// The manifest lists this file with no chunks; it was skipped. Size is
    /// always `0`.
    FileWithNoChunks(String, u64),
    /// The file's relative path could not be resolved to a location on
    /// disk.
    CouldNotResolvePath(String, u64),
    /// The file was created/resized (`set_len`) to its expected size.
    FileAllocationSuccess(String, u64),
}
