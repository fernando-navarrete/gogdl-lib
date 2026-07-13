use std::io;

use reqwest::StatusCode;
use thiserror::Error;

use crate::{auth::AuthorizedFetchError, client::ClientError, games::GamesError};

#[derive(Error, Debug)]
pub enum SecureLinksError {
    #[error("not authenticated")]
    NotAuthenticated,

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

    #[error("Could not get owned games: {0}")]
    GamesError(#[from] GamesError),

    #[error("Product not owned: {0}")]
    ProductNotOwned(String),

    #[error("Stream error")]
    StreamError(),

    #[error("Request timed out")]
    Timeout,
}

impl From<ClientError> for SecureLinksError {
    fn from(value: ClientError) -> Self {
        match value {
            ClientError::UrlParseError(parse_error) => SecureLinksError::UrlParseError(parse_error),
            ClientError::NetworkError(error) => SecureLinksError::NetworkError(error),
            ClientError::Http { status, body } => match status {
                StatusCode::UNAUTHORIZED => SecureLinksError::Unauthorized,
                _ => SecureLinksError::Http { status, body },
            },
            ClientError::DecodeError(error) => SecureLinksError::DecodeError(error),
            ClientError::DeflateError(error) => SecureLinksError::DeflateError(error),
            ClientError::StreamError() => SecureLinksError::StreamError(),
            ClientError::Timeout => SecureLinksError::Timeout,
        }
    }
}

impl From<AuthorizedFetchError> for SecureLinksError {
    fn from(err: AuthorizedFetchError) -> Self {
        match err {
            AuthorizedFetchError::NotAuthenticated => SecureLinksError::NotAuthenticated,
            AuthorizedFetchError::Client(err) => SecureLinksError::from(err),
        }
    }
}
