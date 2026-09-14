use thiserror::Error;

use crate::{ClientError, DepotError, GamesError};

#[derive(Debug, Error)]
pub enum SavesError {
    #[error("Client error: {0}")]
    ClientError(#[from] ClientError),

    #[error("Games error: {0}")]
    GamesError(#[from] GamesError),

    #[error("Depot error: {0}")]
    DepotError(#[from] DepotError),

    #[error("Build not found")]
    BuildNotFound,
}
