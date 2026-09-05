use thiserror::Error;

use crate::{
    client::ClientError, depot::DepotError, downloader::DownloadError, games::GamesError,
    secure_links::SecureLinksError,
};

/// The top-level error type returned by every fallible [`crate::GogDl`]
/// method. Wraps whichever per-layer error enum the failure originated in;
/// there is no flattened variant, so an auth failure typically arrives as
/// e.g. `GogDlError::ClientError(ClientError::AuthError(_))`.
#[derive(Error, Debug)]
pub enum GogDlError {
    /// A catalog lookup failed — see [`GamesError`].
    #[error("Game error: {0}")]
    GameError(#[from] GamesError),

    /// A depot/build-metadata lookup failed — see [`DepotError`].
    #[error("Depot error: {0}")]
    DepotError(#[from] DepotError),

    /// Fetching or resolving a CDN secure link failed — see
    /// [`SecureLinksError`].
    #[error("SecureLinks error: {0}")]
    SecureLinksError(#[from] SecureLinksError),

    /// A download/repair/verify operation failed — see [`DownloadError`].
    #[error("Download error: {0}")]
    DownloadError(#[from] DownloadError),

    /// The underlying HTTP client or auth layer failed — see
    /// [`ClientError`].
    #[error("Client error: {0}")]
    ClientError(#[from] ClientError),
}
