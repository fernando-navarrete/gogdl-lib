mod auth;
mod client;
mod constants;
mod downloader;
mod games;
mod gogdl;

pub use downloader::Depot;
pub use downloader::DepotInfo;
pub use gogdl::GogDl;
pub use gogdl::GogDlError;
pub use reqwest::Client;
