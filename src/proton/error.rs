use thiserror::Error;

use crate::ClientError;

#[derive(Debug, Error)]
pub enum ProtonError {
    #[error("Client error: {0}")]
    ClientError(#[from] ClientError),
}
