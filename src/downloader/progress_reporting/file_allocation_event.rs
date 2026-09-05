/// One file's result during the file-allocation stage
/// (wrapped in
/// [`DownloadStageEvent::FileAllocationStage`](crate::DownloadStageEvent::FileAllocationStage)).
/// Every payload is `(relative path, expected uncompressed size)`.
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
