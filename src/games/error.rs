use std::io;

use reqwest::StatusCode;
use thiserror::Error;

use crate::client::ClientError;

#[derive(Error, Debug)]
pub enum GamesError {
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

    #[error("Client error: {0}")]
    ClientError(#[from] ClientError),
}
