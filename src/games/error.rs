use std::io;

use reqwest::StatusCode;
use thiserror::Error;

use crate::client::ClientError;

#[derive(Error, Debug)]
pub enum GamesError {
    #[error("Unauthorized")]
    Unauthorized,

    #[error("Url parse error: {0}")]
    UrlParseError(#[from] url::ParseError),

    #[error("Network error: {0}")]
    NetworkError(#[from] reqwest::Error),

    #[error("Http error: {body}, status: {status}")]
    Http { status: StatusCode, body: String },

    #[error("Decode error: {0}")]
    DecodeError(#[from] serde_json::Error),

    #[error("Deflate error: {0}")]
    DeflateError(#[from] io::Error),

    #[error("Product not a game")]
    ProductNotAGame,
}

impl From<ClientError> for GamesError {
    fn from(value: ClientError) -> Self {
        match value {
            ClientError::UrlParseError(parse_error) => GamesError::UrlParseError(parse_error),
            ClientError::NetworkError(error) => GamesError::NetworkError(error),
            ClientError::Http { status, body } => match status {
                StatusCode::UNAUTHORIZED => GamesError::Unauthorized,
                _ => GamesError::Http { status, body },
            },
            ClientError::DecodeError(error) => GamesError::DecodeError(error),
            ClientError::DeflateError(error) => GamesError::DeflateError(error),
        }
    }
}
