use thiserror::Error;

use crate::fs::FileSystemError;
use crate::{ClientError, DepotError, GamesError};

/// Errors from the cloud saves operations,
/// [`GogDl::get_save_files`](crate::GogDl::get_save_files),
/// [`GogDl::get_remote_config`](crate::GogDl::get_remote_config),
/// [`GogDl::download_save_files`](crate::GogDl::download_save_files) and
/// [`GogDl::upload_save_files`](crate::GogDl::upload_save_files).
#[derive(Debug, Error)]
pub enum SavesError {
    /// Reading the session's refresh token, the token exchange with
    /// `auth.gog.com`, or a request to `cloudstorage.gog.com` (listing,
    /// downloading or uploading) failed — see [`ClientError`]. Notably wraps
    /// [`AuthError::NotAuthenticated`](crate::AuthError::NotAuthenticated) or
    /// [`AuthError::TokenExpired`](crate::AuthError::TokenExpired) if there is
    /// no usable session, or an
    /// [`HttpError`](crate::ClientError::HttpError) if GOG rejects a
    /// request. No request is retried.
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

    /// Reading a local save file, writing a downloaded one, or walking the
    /// save directory failed.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Creating or resolving a path under the caller's save directory
    /// failed, including a cloud file name that would resolve outside it.
    /// The inner `FileSystemError` type is crate-private; only its `Display`
    /// output (via this variant's own message) is visible here.
    #[error("File system error: {0}")]
    FileSystemError(#[from] FileSystemError),

    /// The MD5 of a downloaded file's bytes does not match the `ETag` GOG
    /// sent with them. The file is not written.
    #[error("Hash mismatch for {name}: expected {expected}, got {actual}")]
    HashMismatch {
        /// The cloud-side name of the file that failed verification.
        name: String,
        /// The hash GOG reported in the `ETag` header.
        expected: String,
        /// The hash computed over the bytes actually received.
        actual: String,
    },

    /// A response header GOG is expected to send was not valid text, or
    /// `X-Object-Meta-LocalLastModified` was not an RFC 3339 timestamp.
    #[error("Invalid header: {0}")]
    InvalidHeader(String),

    /// A file name in the cloud listing is not of the form
    /// `saves/<location>/<path>`, so no local path can be derived from it.
    #[error("Invalid save file name: {0}")]
    InvalidSaveFileName(String),
}
