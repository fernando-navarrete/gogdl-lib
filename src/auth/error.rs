use std::io;

use reqwest::StatusCode;
use thiserror::Error;

use crate::client::ClientError;

#[derive(Error, Debug)]
pub enum AuthError {
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
}

impl From<ClientError> for AuthError {
    fn from(err: ClientError) -> Self {
        match err {
            ClientError::UrlParseError(parse_error) => AuthError::UrlParseError(parse_error),
            ClientError::NetworkError(error) => AuthError::NetworkError(error),
            ClientError::Http { status, body } => match status {
                StatusCode::UNAUTHORIZED => AuthError::Unauthorized,
                _ => AuthError::Http { status, body },
            },
            ClientError::DecodeError(error) => AuthError::DecodeError(error),
            ClientError::DeflateError(error) => AuthError::DeflateError(error),
        }
    }
}
