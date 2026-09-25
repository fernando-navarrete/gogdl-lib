use crate::downloader::DownloadEvent;
use crate::downloader::progress_reporting::FileAllocationEvent;
use crate::downloader::progress_reporting::FileSizeVerificationEvent;
use crate::downloader::progress_reporting::VerificationEvent;

/// The top-level progress event sent by
/// [`GogDl::download_game`](crate::GogDl::download_game) and
/// [`GogDl::repair_game`](crate::GogDl::repair_game), tagging which pipeline
/// stage an inner event belongs to.
///
/// `download_game` emits, in order: `FileSizeVerificationStage`,
/// `FileAllocationStage` (then `FileAllocationError` and an early return if
/// any file failed to allocate), `DownloadStage`. `repair_game` inserts a
/// `VerificationStage` pass between allocation and the download stage, and
/// only feeds units that fail verification into `DownloadStage`. Neither
/// stage set is documented anywhere else — this ordering is the contract.
///
/// `DownloadStage` may stop partway through: the first chunk that fails
/// terminally aborts the rest of that stage, so a failed call doesn't imply
/// every unit was attempted. See the batch-abort note on
/// [`GogDl::download_game`](crate::GogDl::download_game).
///
/// # Constructing in tests
///
/// Every variant is public, so build them directly. The type does not implement
/// `Debug` or `PartialEq`; match with `matches!` instead of `assert_eq!`.
///
/// ```
/// use gogdl_lib::{DownloadEvent, DownloadStageEvent};
///
/// let event = DownloadStageEvent::DownloadStage(DownloadEvent::Downloading);
/// assert!(matches!(
///     event,
///     DownloadStageEvent::DownloadStage(DownloadEvent::Downloading)
/// ));
/// ```
pub enum DownloadStageEvent {
    /// A file's on-disk size was checked against its expected size, before
    /// any allocation or transfer. See [`FileSizeVerificationEvent`].
    FileSizeVerificationStage(FileSizeVerificationEvent),
    /// A file that failed size verification is being pre-allocated
    /// (`set_len` to its final size) before the transfer stage. See
    /// [`FileAllocationEvent`].
    FileAllocationStage(FileAllocationEvent),
    /// `repair_game` only: an on-disk chunk was checksummed against the
    /// manifest to decide whether it needs re-downloading. See
    /// [`VerificationEvent`].
    VerificationStage(VerificationEvent),
    /// At least one file failed allocation; the operation is aborting with
    /// [`DownloadError::FileAllocationError`](crate::DownloadError::FileAllocationError)
    /// (this event and that error always arrive together).
    FileAllocationError(),
    /// The chunk-transfer stage. See [`DownloadEvent`].
    DownloadStage(DownloadEvent),
}
