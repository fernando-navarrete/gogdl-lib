use std::io;

use reqwest::StatusCode;
use thiserror::Error;

use crate::{auth::AuthError, client::ClientError};

#[derive(Error, Debug)]
pub enum DepotError {
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

    #[error("Auth error: {0}")]
    AuthError(#[from] AuthError),

    #[error("Unknown error")]
    Unknown,
}

impl From<ClientError> for DepotError {
    fn from(err: ClientError) -> Self {
        match err {
            ClientError::UrlParseError(parse_error) => DepotError::UrlParseError(parse_error),
            ClientError::NetworkError(error) => DepotError::NetworkError(error),
            ClientError::Http { status, body } => match status {
                StatusCode::UNAUTHORIZED => DepotError::AuthError(AuthError::Unauthorized),
                _ => DepotError::Http { status, body },
            },
            ClientError::DecodeError(error) => DepotError::DecodeError(error),
            ClientError::DeflateError(error) => DepotError::DeflateError(error),
            ClientError::AuthError(auth_error) => DepotError::AuthError(auth_error),
            ClientError::Unknown => DepotError::Unknown,
        }
    }
}
