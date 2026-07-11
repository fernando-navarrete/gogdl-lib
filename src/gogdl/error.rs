use thiserror::Error;

use crate::{
    auth::AuthError, depot::DepotError, downloader::DownloadError, games::GamesError,
    saves::SavesError, secure_links::SecureLinksError,
};

#[derive(Error, Debug)]
pub enum GogDlError {
    #[error("AuthError: {0}")]
    AuthError(#[from] AuthError),

    #[error("Game error: {0}")]
    GameError(#[from] GamesError),

    #[error("Depot error: {0}")]
    DepotError(#[from] DepotError),

    #[error("SecureLinks error: {0}")]
    SecureLinksError(#[from] SecureLinksError),

    #[error("Download error: {0}")]
    DownloadError(#[from] DownloadError),

    #[error("Saves error: {0}")]
    SavesError(#[from] SavesError),
}
