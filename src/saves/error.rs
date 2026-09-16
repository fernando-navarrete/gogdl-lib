use thiserror::Error;

use crate::{ClientError, DepotError, GamesError};

/// Errors from the cloud saves operations,
/// [`GogDl::get_save_files`](crate::GogDl::get_save_files) and
/// [`GogDl::get_remote_config`](crate::GogDl::get_remote_config).
#[derive(Debug, Error)]
pub enum SavesError {
    /// Reading the session's refresh token, the token exchange with
    /// `auth.gog.com`, or the listing request to `cloudstorage.gog.com`
    /// failed — see [`ClientError`]. Notably wraps
    /// [`AuthError::NotAuthenticated`](crate::AuthError::NotAuthenticated) or
    /// [`AuthError::TokenExpired`](crate::AuthError::TokenExpired) if there is
    /// no usable session, or an
    /// [`HttpError`](crate::ClientError::HttpError) if GOG rejects either
    /// request. Neither request is retried.
    #[error("Client error: {0}")]
    ClientError(#[from] ClientError),

    /// Listing the game's builds failed — see [`GamesError`].
    #[error("Games error: {0}")]
    GamesError(#[from] GamesError),

    /// Fetching the selected build's metadata (which carries the game's
    /// `client_id`/`client_secret`) failed — see [`DepotError`].
    #[error("Depot error: {0}")]
    DepotError(#[from] DepotError),

    /// No build of the requested game has a
    /// [`version_name`](crate::GameBuild::version_name) equal to the given
    /// `build_name`.
    #[error("Build not found")]
    BuildNotFound,

    /// The game's [`RemoteConfig`](crate::RemoteConfig) declares no cloud
    /// storage for Windows — it has either no Windows section at all, or
    /// one with no `cloudStorage` block. Only ever returned by
    /// [`RemoteConfig::get_locations`](crate::RemoteConfig::get_locations),
    /// never by a request.
    #[error("Cloud storage not supported")]
    CloudStorageNotSupported,
}
