mod auth;
mod client;
mod constants;
mod depot;
mod downloader;
mod games;
mod gogdl;
mod secure_links;

pub use depot::ProductDetails;
pub use downloader::DownloadDetail;
pub use downloader::DownloadEvent;
pub use downloader::DownloadJobEvent;
pub use downloader::DownloadStage;
pub use downloader::DownloadableFiles;
pub use downloader::DownloadableProduct;
pub use downloader::FileAllocationEvent;
pub use downloader::FileVerifyEvent;
pub use downloader::RepairDetail;
pub use downloader::RepairEvent;
pub use downloader::RepairStage;
pub use downloader::VerifyChunksEvent;
pub use downloader::VerifyEvent;
pub use games::GameBuilds;
pub use games::GameDetails;
pub use games::GameLinks;
pub use games::GameScreenshots;
pub use games::GameSummary;
pub use games::OwnedGames;
pub use gogdl::GogDl;
pub use gogdl::GogDlError;
pub use reqwest::Client;
pub use secure_links::SecureLinks;

/// The event types emitted on the channels passed into `GogDl`'s
/// `download_files`, `repair_files`, and `verify_files` methods, grouped
/// under one path. The same types remain available individually at the
/// crate root (e.g. `gogdl_lib2::DownloadEvent`) for existing callers.
pub mod events {
    pub use crate::downloader::{
        DownloadDetail, DownloadEvent, DownloadJobEvent, DownloadStage, FileAllocationEvent,
        FileVerifyEvent, RepairDetail, RepairEvent, RepairStage, VerifyChunksEvent, VerifyEvent,
    };
}
