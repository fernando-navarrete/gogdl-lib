use crate::downloader::progress_reporting::FileAllocationEvent;
use crate::downloader::progress_reporting::FileSizeVerificationEvent;

pub enum DownloadStageEvent {
    FileSizeVerificationStage(FileSizeVerificationEvent),
    FileAllocationStage(FileAllocationEvent),
}
