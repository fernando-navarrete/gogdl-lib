use std::{io, num::ParseIntError};

use reqwest::StatusCode;
use thiserror::Error;

use crate::{client::ClientError, games::GamesError};

/// Errors resolving a CDN secure link for a chunk download.
///
/// `Http`, `UrlParseError`, `NetworkError`, `DecodeError` and `DeflateError`
/// are currently unconstructible — every network call in this layer goes
/// through [`ClientError`](crate::ClientError), which is what actually
/// arrives on failure. They're kept for API stability; don't rely on
/// matching them.
#[derive(Error, Debug)]
pub enum SecureLinksError {
    /// Unconstructible — see the enum-level note.
    #[error("Url parse error: {0}")]
    UrlParseError(#[from] url::ParseError),

    /// Unconstructible — see the enum-level note.
    #[error("Network error: {0}")]
    NetworkError(#[from] reqwest::Error),

    /// Unconstructible — see the enum-level note.
    #[error("Http error: {body}, status: {status}")]
    Http {
        /// The response status code.
        status: StatusCode,
        /// The raw response body.
        body: String,
    },

    /// Unconstructible — see the enum-level note.
    #[error("Decode error: {0}")]
    DecodeError(#[from] serde_json::Error),

    /// Unconstructible — see the enum-level note.
    #[error("Deflate error: {0}")]
    DeflateError(#[from] io::Error),

    /// Checking the account's owned-games list (required before fetching a
    /// secure link) failed — see [`GamesError`](crate::GamesError).
    #[error("Could not get owned games: {0}")]
    GamesError(#[from] GamesError),

    /// The requested game ID is not owned by the authenticated account.
    #[error("Product not owned: {0}")]
    ProductNotOwned(String),

    /// The secure-link response contained no usable URL format.
    #[error("No secure link available")]
    NoSecureLink,

    /// The actual failure path for this layer today — see
    /// [`ClientError`](crate::ClientError).
    #[error("Client error: {0}")]
    ClientError(#[from] ClientError),

    /// The game ID string could not be parsed as an integer.
    #[error("Incorrect game id: {0}, parse error: {1}")]
    IncorrectGameId(String, ParseIntError),
}
