use std::num::ParseIntError;

use thiserror::Error;

use crate::{client::ClientError, games::GamesError};

/// Errors resolving a CDN secure link for a chunk download.
///
/// `#[non_exhaustive]`: a `match` outside the crate needs a wildcard arm.
#[derive(Error, Debug)]
#[non_exhaustive]
pub enum SecureLinksError {
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

    /// A request failed in transit, was rejected, or didn't decode — see
    /// [`ClientError`](crate::ClientError).
    #[error("Client error: {0}")]
    ClientError(#[from] ClientError),

    /// The game ID string could not be parsed as an integer.
    #[error("Incorrect game id: {0}, parse error: {1}")]
    IncorrectGameId(String, ParseIntError),
}
