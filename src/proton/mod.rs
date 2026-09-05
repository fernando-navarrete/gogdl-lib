mod error;
mod github_asset;
mod proton_download_event;
mod proton_downloader;
mod proton_ge_release;
mod proton_ge_releases_page;
mod proton_manager;

use proton_downloader::ProtonDownloader;

pub use error::ProtonError;
pub use github_asset::GithubAsset;
pub use proton_download_event::ProtonDownloadEvent;
pub use proton_ge_release::ProtonGeRelease;
pub use proton_ge_releases_page::ProtonGeReleasesPage;
pub use proton_manager::ProtonManager;
