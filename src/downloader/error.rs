use std::io;

use reqwest::StatusCode;
use thiserror::Error;

use crate::{
    client::ClientError, depot::DepotError, fs::FileSystemError, games::GamesError,
    secure_links::SecureLinksError,
};

/// Errors from [`GogDl::verify_files`](crate::GogDl::verify_files),
/// [`GogDl::download_game`](crate::GogDl::download_game) and
/// [`GogDl::repair_game`](crate::GogDl::repair_game), and the catalog/depot
/// lookups they depend on.
///
/// `UrlParseError`, `NetworkError`, `Http` and `DecodeError` are currently
/// unconstructible — every network call in this crate goes through
/// [`ClientError`](crate::ClientError), which is what actually arrives on
/// transport failure. They're kept for API stability; don't rely on matching
/// them. `DeflateError` *is* constructed directly by this layer (zlib
/// decode/shutdown failures during a chunk download), unlike its
/// same-named, unconstructible siblings on the other error enums.
#[derive(Error, Debug)]
pub enum DownloadError {
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

    /// Zlib decoding failed while streaming or finalizing a chunk, or the
    /// decompressed byte count didn't match the chunk's declared size.
    #[error("Deflate error: {0}")]
    DeflateError(io::Error),

    /// A catalog lookup failed — see [`GamesError`](crate::GamesError).
    #[error("Game error: {0}")]
    GamesError(#[from] GamesError),

    /// A depot/build-metadata lookup failed — see
    /// [`DepotError`](crate::DepotError).
    #[error("Depot error: {0}")]
    DepotError(#[from] DepotError),

    /// No build matching the requested name exists for this game.
    #[error("Build not found")]
    BuildNotFound,

    /// Pre-allocating one or more files on disk failed (e.g. permissions or
    /// a disk error other than being full — that case is caught earlier by
    /// [`NotEnoughFreeSpace`](Self::NotEnoughFreeSpace)).
    #[error("File allocation error")]
    FileAllocationError,

    /// Resolving or invalidating a CDN secure link failed — see
    /// [`SecureLinksError`](crate::SecureLinksError).
    #[error("Secure links error: {0}")]
    SecureLinksError(#[from] SecureLinksError),

    /// Path resolution or a filesystem operation under the install directory
    /// failed. The inner `FileSystemError` type is crate-private; only its
    /// `Display` output (via this variant's own message) is visible here.
    #[error("File system error: {0}")]
    FileSystemError(#[from] FileSystemError),

    /// The underlying HTTP client or auth layer failed, or every retry
    /// attempt for a chunk was exhausted — see
    /// [`ClientError`](crate::ClientError).
    #[error("Client error: {0}")]
    ClientError(#[from] ClientError),

    /// [`GogDl::verify_files`](crate::GogDl::verify_files) found this many
    /// chunks missing, unreadable, or failing their checksum.
    #[error("Chunk integrity check failed: incomplete chunks: {0}")]
    ChunkIntegrityCheckFailed(usize),

    /// Free space on the destination disk couldn't be determined (e.g. no
    /// mounted disk matches the resolved install path).
    #[error("Could not resolve free space")]
    CouldNotResolveFreeSpace,

    /// The transfer needs more free space than is available on the
    /// destination disk.
    #[error("Not enough free space")]
    NotEnoughFreeSpace,

    /// A chunk's decompressed MD5 didn't match the manifest after every
    /// retry was exhausted.
    #[error(
        "Chunk hash mismatch during download: file={path} offset={offset} expected={expected} actual={actual}"
    )]
    ChunkHashMismatch {
        /// The path of the file being downloaded.
        path: String,
        /// The offset of the chunk within the file.
        offset: u64,
        /// The expected MD5 hash of the chunk.
        expected: String,
        /// The actual MD5 hash of the chunk.
        actual: String,
    },
}
