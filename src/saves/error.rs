use std::io;

use reqwest::StatusCode;
use thiserror::Error;

use crate::{
    auth::{AuthError, AuthorizedFetchError},
    client::ClientError,
    depot::DepotError,
    games::GamesError,
};

#[derive(Error, Debug)]
pub enum SavesError {
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

    #[error("IO error: {0}")]
    Io(#[from] io::Error),

    #[error("Stream error")]
    StreamError(),

    #[error("Could not get game info: {0}")]
    Games(#[from] GamesError),

    #[error("Could not get build metadata: {0}")]
    Depot(#[from] DepotError),

    #[error("Auth error: {0}")]
    Auth(#[from] AuthError),

    #[error("No build found for this game")]
    BuildNotFound,

    #[error("Malformed cloud-storage path placeholder: {0:?}")]
    MalformedRemotePath(String),

    #[error("Unknown known-folder key {0:?} in remote config")]
    UnknownFolderKey(String),

    #[error("Invalid timestamp {0:?}: not a valid RFC3339 date")]
    InvalidTimestamp(String),

    #[error("Malformed save-file listing line: {0:?}")]
    MalformedSaveLine(String),

    #[error("Hash mismatch: expected {expected}, got {actual}")]
    HashMismatch { expected: String, actual: String },
}

impl From<ClientError> for SavesError {
    fn from(err: ClientError) -> Self {
        match err {
            ClientError::UrlParseError(parse_error) => SavesError::UrlParseError(parse_error),
            ClientError::NetworkError(error) => SavesError::NetworkError(error),
            ClientError::Http { status, body } => match status {
                StatusCode::UNAUTHORIZED => SavesError::Unauthorized,
                _ => SavesError::Http { status, body },
            },
            ClientError::DecodeError(error) => SavesError::DecodeError(error),
            ClientError::DeflateError(error) => SavesError::Io(error),
            ClientError::StreamError() => SavesError::StreamError(),
        }
    }
}

impl From<AuthorizedFetchError> for SavesError {
    fn from(err: AuthorizedFetchError) -> Self {
        match err {
            AuthorizedFetchError::NotAuthenticated => SavesError::NotAuthenticated,
            AuthorizedFetchError::Client(err) => SavesError::from(err),
        }
    }
}
