mod auth;
mod client;
mod constants;
mod depot;
mod downloader;
mod games;
mod gogdl;
mod secure_links;

pub use depot::Depot;
pub use depot::DepotInfo;
pub use gogdl::GogDl;
pub use gogdl::GogDlError;
pub use reqwest::Client;
