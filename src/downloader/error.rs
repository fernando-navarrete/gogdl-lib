use std::io;

use reqwest::StatusCode;
use thiserror::Error;

use crate::{
    client::ClientError, depot::DepotError, games::GamesError, secure_links::SecureLinksError,
};

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

    #[error("Disk allocation error")]
    DiskAllocationError,

    #[error("Secure links error: {0}")]
    SecureLinksError(#[from] SecureLinksError),

    #[error("Stream error")]
    StreamError(),

    #[error("Request timed out")]
    Timeout,
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
            ClientError::StreamError() => DownloadError::StreamError(),
            ClientError::Timeout => DownloadError::Timeout,
        }
    }
}
