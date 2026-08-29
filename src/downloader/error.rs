use std::io;

use reqwest::StatusCode;
use thiserror::Error;

use crate::{client::ClientError, depot::DepotError, games::GamesError};

#[derive(Error, Debug)]
pub enum DownloadError {
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

    #[error("Deflate error: {0}")]
    DeflateError(#[from] io::Error),

    #[error("Game error: {0}")]
    GamesError(#[from] GamesError),

    #[error("Depot error: {0}")]
    DepotError(#[from] DepotError),

    #[error("Build not found")]
    BuildNotFound,

    #[error("File allocation error")]
    FileAllocationError,
}

impl From<ClientError> for DownloadError {
    fn from(err: ClientError) -> Self {
        match err {
            ClientError::UrlParseError(parse_error) => DownloadError::UrlParseError(parse_error),
            ClientError::NetworkError(error) => DownloadError::NetworkError(error),
            ClientError::Http { status, body } => match status {
                StatusCode::UNAUTHORIZED => DownloadError::Unauthorized,
                _ => DownloadError::Http { status, body },
            },
            ClientError::DecodeError(error) => DownloadError::DecodeError(error),
            ClientError::DeflateError(error) => DownloadError::DeflateError(error),
        }
    }
}
