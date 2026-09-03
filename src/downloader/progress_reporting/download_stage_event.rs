use crate::downloader::DownloadEvent;
use crate::downloader::progress_reporting::FileAllocationEvent;
use crate::downloader::progress_reporting::FileSizeVerificationEvent;
use crate::downloader::progress_reporting::VerificationEvent;

pub enum DownloadStageEvent {
    FileSizeVerificationStage(FileSizeVerificationEvent),
    FileAllocationStage(FileAllocationEvent),
    VerificationStage(VerificationEvent),
    FileAllocationError(),
    DownloadStage(DownloadEvent),
}
