//! The event types emitted on the channels passed into `Downloader`'s
//! `download`, `repair_download`, and `verify` entry points. Kept separate
//! from the engine (`downloader.rs`) since this is the public event API
//! callers depend on, not engine implementation detail.

pub enum VerifyChunksEvent {
    PathResolveError(String),
    FileNotFound(String),
    ChecksumCalculationError(String),
    ChunkChecksumMismatch(String),
}

pub enum FileAllocationEvent {
    PathResolveError(String),
    FileAllocated(String),
    AllocationError(String),
    FileSizeError(String),
    FileOk,
}

pub enum FileVerifyEvent {
    FileNotFound(String),
    CouldNotResolvePath(String),
    CouldNotReadFileSize(String),
    SizeMismatch(String, u64, u64),
    ChecksumMismatch(String),
    FileOk,
}

pub enum VerifyEvent {
    CouldNotResolvePath(String),
    FileNotFound(String),
    ChunkChecksumMismatch(String),
    ChunkOk,
}

/// Emitted by the download stage of `repair_download`.
pub enum DownloadEvent {
    /// Sent once at the start of the download stage: the total decoded
    /// bytes and chunk count queued for download. Clients can track
    /// progress as `downloaded = Σ Progress.bytes` and
    /// `remaining = Total.bytes - downloaded`.
    Total { bytes: u64, chunks: usize },
    /// An incremental delta of decoded bytes written for a chunk still in
    /// flight. Sent as each network read is decoded and written to disk.
    Progress { bytes: u64 },
    /// A chunk was downloaded and written successfully in full.
    ChunkDownloaded { path: String },
    /// No usable secure link was available for this chunk's product.
    SecureLinkError(String),
    /// The chunk failed to download (network error, stream closed, or not authenticated).
    DownloadError(String),
    /// The chunk downloaded but couldn't be written to disk at its offset.
    WriteError(String),
    /// The destination path for the chunk couldn't be resolved.
    PathResolveError(String),
}

/// Which stage of `repair_download` a `RepairEvent` originated from.
pub enum RepairStage {
    VerifyingFiles,
    Allocating,
    VerifyingChunks,
    Downloading,
}

/// The stage-specific event wrapped by a `RepairEvent`.
pub enum RepairDetail {
    FileVerify(FileVerifyEvent),
    Allocation(FileAllocationEvent),
    ChunkVerify(VerifyChunksEvent),
    Download(DownloadEvent),
}

/// A single status update from `repair_download`, tagged with the stage it
/// came from so callers tracking a multi-stage repair can tell them apart
/// on one channel.
pub struct RepairEvent {
    pub stage: RepairStage,
    pub detail: RepairDetail,
}

/// Which stage of `Downloader::download` a `DownloadJobEvent` originated from.
pub enum DownloadStage {
    Allocating,
    Downloading,
}

/// The stage-specific event wrapped by a `DownloadJobEvent`.
pub enum DownloadDetail {
    Allocation(FileAllocationEvent),
    Download(DownloadEvent),
}

/// A single status update from `Downloader::download`, tagged with the
/// stage it came from so callers tracking a multi-stage download can tell
/// them apart on one channel.
pub struct DownloadJobEvent {
    pub stage: DownloadStage,
    pub detail: DownloadDetail,
}
