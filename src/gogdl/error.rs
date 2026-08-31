use thiserror::Error;

use crate::{
    client::ClientError, depot::DepotError, downloader::DownloadError, games::GamesError,
    secure_links::SecureLinksError,
};

#[derive(Error, Debug)]
pub enum GogDlError {
    #[error("Game error: {0}")]
    GameError(#[from] GamesError),

    #[error("Depot error: {0}")]
    DepotError(#[from] DepotError),

    #[error("SecureLinks error: {0}")]
    SecureLinksError(#[from] SecureLinksError),

    #[error("Download error: {0}")]
    DownloadError(#[from] DownloadError),

    #[error("Client error: {0}")]
    ClientError(#[from] ClientError),
}
