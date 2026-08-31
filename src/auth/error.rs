use thiserror::Error;

#[derive(Error, Debug)]
pub enum AuthError {
    #[error("Not yet authenticated")]
    NotAuthenticated,

    #[error("Auth token expired locally")]
    TokenExpired,

    #[error("Could not decode local auth token: {0}")]
    AuthDecodeError(serde_json::Error),

    #[error("Could not encode token: {0}")]
    AuthEncodeError(serde_json::Error),

    #[error("Client error: {inner}")]
    ClientError { inner: String },
}
