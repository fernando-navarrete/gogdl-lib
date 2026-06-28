use thiserror::Error;

use crate::auth::AuthError;

#[derive(Error, Debug)]
pub enum GogDlError {
    #[error("AuthError: {0}")]
    AuthError(#[from] AuthError),
}
