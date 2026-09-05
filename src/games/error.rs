use std::io;

use reqwest::StatusCode;
use thiserror::Error;

use crate::client::ClientError;

/// Errors from catalog lookups (owned games, game details/builds/links/
/// summary/screenshots).
///
/// `Http`, `UrlParseError`, `NetworkError`, `DecodeError` and `DeflateError`
/// are currently unconstructible — every network call in this layer goes
/// through [`ClientError`](crate::ClientError), which is what actually
/// arrives on failure. They're kept for API stability; don't rely on
/// matching them.
#[derive(Error, Debug)]
pub enum GamesError {
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

    /// The requested product ID is not a game (e.g. it's a DLC). Only
    /// [`crate::GogDl::get_game_details`] currently distinguishes this from
    /// a decode failure — the other catalog getters don't.
    #[error("Product not a game")]
    ProductNotAGame,

    /// The actual failure path for this layer today — see
    /// [`ClientError`](crate::ClientError).
    #[error("Client error: {0}")]
    ClientError(#[from] ClientError),
}
