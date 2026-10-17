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
///
/// `#[non_exhaustive]`: a `match` outside the crate needs a wildcard arm.
#[derive(Error, Debug)]
#[non_exhaustive]
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

    /// The requested product ID is not a game (e.g. it's a DLC or a pack).
    /// Returned by [`crate::GogDl::get_game_details`],
    /// [`get_game_builds`](crate::GogDl::get_game_builds),
    /// [`get_game_links`](crate::GogDl::get_game_links) and
    /// [`get_game_summary`](crate::GogDl::get_game_summary), and through the
    /// last-named builds lookup by the downloads and cloud saves methods.
    #[error("Product not a game")]
    ProductNotAGame,

    /// The actual failure path for this layer today — see
    /// [`ClientError`](crate::ClientError).
    #[error("Client error: {0}")]
    ClientError(#[from] ClientError),
}
