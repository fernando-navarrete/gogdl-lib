mod auth;
mod client;
mod constants;
mod depot;
mod games;
mod gogdl;

pub use depot::Depot;
pub use depot::DepotInfo;
pub use gogdl::GogDl;
pub use gogdl::GogDlError;
pub use reqwest::Client;
