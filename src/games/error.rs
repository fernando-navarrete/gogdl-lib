use thiserror::Error;

use crate::client::ClientError;

/// Errors from catalog lookups (owned games, game details/builds/links/
/// summary/screenshots).
///
/// `#[non_exhaustive]`: a `match` outside the crate needs a wildcard arm.
#[derive(Error, Debug)]
#[non_exhaustive]
pub enum GamesError {
    /// The requested product ID is not a game (e.g. it's a DLC or a pack).
    /// Returned by [`crate::GogDl::get_game_details`],
    /// [`get_game_builds`](crate::GogDl::get_game_builds),
    /// [`get_game_links`](crate::GogDl::get_game_links) and
    /// [`get_game_summary`](crate::GogDl::get_game_summary), and through the
    /// last-named builds lookup by the downloads and cloud saves methods.
    #[error("Product not a game")]
    ProductNotAGame,

    /// A request failed in transit, was rejected, or didn't decode — see
    /// [`ClientError`](crate::ClientError).
    #[error("Client error: {0}")]
    ClientError(#[from] ClientError),
}
