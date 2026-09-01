use thiserror::Error;

use crate::client::auth::AuthError;

#[derive(Error, Debug)]
pub enum ClientError {
    #[error("Url parse error: {0}")]
    UrlParseError(#[from] url::ParseError),

    #[error("Network error: {0}")]
    NetworkError(#[from] reqwest::Error),

    #[error("Http error: status: {status}, body: {body}")]
    HttpError {
        status: reqwest::StatusCode,
        body: String,
    },

    #[error("Deserialization error: {0}")]
    DeserializationError(#[from] serde_json::Error),

    #[error("Decode error: {0}")]
    DecodeError(std::io::Error),

    #[error("Chunk stream callback error: {0}")]
    ChunkStreamCallbackError(std::io::Error),

    #[error("Max retires reached")]
    MaxRetriesReached,

    #[error("Auth error: {0}")]
    AuthError(#[from] AuthError),

    #[error("Secure links error: {inner}")]
    SecureLinksError { inner: String },
}
