use std::{io, num::ParseIntError};

use reqwest::StatusCode;
use thiserror::Error;

use crate::{client::ClientError, games::GamesError};

#[derive(Error, Debug)]
pub enum SecureLinksError {
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

    #[error("Could not get owned games: {0}")]
    GamesError(#[from] GamesError),

    #[error("Product not owned: {0}")]
    ProductNotOwned(String),

    #[error("No secure link available")]
    NoSecureLink,

    #[error("Client error: {0}")]
    ClientError(#[from] ClientError),

    #[error("Incorrect game id: {0}, parse error: {1}")]
    IncorrectGameId(String, ParseIntError),
}
