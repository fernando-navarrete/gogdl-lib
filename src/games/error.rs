use reqwest::StatusCode;
use thiserror::Error;

use crate::client::ClientError;

#[derive(Error, Debug)]
pub enum GamesError {
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
}

impl From<ClientError> for GamesError {
    fn from(value: ClientError) -> Self {
        match value {
            ClientError::UrlParseError(parse_error) => GamesError::UrlParseError(parse_error),
            ClientError::NetworkError(error) => GamesError::NetworkError(error),
            ClientError::Http { status, body } => match status {
                StatusCode::UNAUTHORIZED => GamesError::NotAuthenticated,
                _ => GamesError::Http { status, body },
            },
        }
    }
}
