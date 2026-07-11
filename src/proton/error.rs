use std::io;

use reqwest::StatusCode;
use thiserror::Error;

use crate::client::ClientError;

#[derive(Error, Debug)]
pub enum ProtonError {
    #[error("Url parse error: {0}")]
    UrlParseError(#[from] url::ParseError),

    #[error("Network error: {0}")]
    NetworkError(#[from] reqwest::Error),

    #[error("Unauthorized")]
    Unauthorized,

    #[error("Http error: {body}, status: {status}")]
    Http { status: StatusCode, body: String },

    #[error("Decode error: {0}")]
    DecodeError(#[from] serde_json::Error),

    #[error("IO error: {0}")]
    Io(#[from] io::Error),

    #[error("Stream error")]
    StreamError(),

    #[error("Release has no downloadable Proton-GE (.gz) asset")]
    NoDownloadAsset,

    #[error("Hash mismatch: expected {expected}, got {actual}")]
    HashMismatch { expected: String, actual: String },
}

impl From<ClientError> for ProtonError {
    fn from(err: ClientError) -> Self {
        match err {
            ClientError::UrlParseError(parse_error) => ProtonError::UrlParseError(parse_error),
            ClientError::NetworkError(error) => ProtonError::NetworkError(error),
            ClientError::Http { status, body } => match status {
                StatusCode::UNAUTHORIZED => ProtonError::Unauthorized,
                _ => ProtonError::Http { status, body },
            },
            ClientError::DecodeError(error) => ProtonError::DecodeError(error),
            ClientError::DeflateError(error) => ProtonError::Io(error),
            ClientError::StreamError() => ProtonError::StreamError(),
        }
    }
}
