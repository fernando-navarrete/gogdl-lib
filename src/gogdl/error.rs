use thiserror::Error;

use crate::{
    auth::AuthError, depot::DepotError, games::GamesError, secure_links::SecureLinksError,
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
}
