use thiserror::Error;

use crate::{auth::AuthError, downloader::DownloadError, games::GamesError};

#[derive(Error, Debug)]
pub enum GogDlError {
    #[error("AuthError: {0}")]
    AuthError(#[from] AuthError),

    #[error("Game error: {0}")]
    GameError(#[from] GamesError),

    #[error("Download error: {0}")]
    DownloadError(#[from] DownloadError),
}
