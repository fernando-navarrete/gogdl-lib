use thiserror::Error;

use crate::ClientError;
use crate::fs::FileSystemError;

/// Errors from listing ([`GogDl::get_proton_releases`](crate::GogDl::get_proton_releases))
/// or downloading ([`GogDl::download_proton_release`](crate::GogDl::download_proton_release))
/// Proton-GE releases.
///
/// Listing releases has no separate decode/auth path of its own, so any
/// failure there (network, a non-2xx status, or a JSON decode error) arrives
/// as [`ClientError`]. Downloading a release can additionally fail with any
/// of the other variants here.
#[derive(Debug, Error)]
pub enum ProtonError {
    /// The underlying HTTP request to GitHub failed — see [`ClientError`].
    /// Notably includes a 403 from a missing `User-Agent` or an exhausted
    /// rate limit; see
    /// [`GogDl::get_proton_releases`](crate::GogDl::get_proton_releases).
    #[error("Client error: {0}")]
    ClientError(#[from] ClientError),

    /// `download_proton_release` only: the release has no asset whose name
    /// ends in `.tar.gz` without containing `aarch64` — i.e. no Linux
    /// x86_64 tarball.
    #[error("No suitable asset found for release: {0}")]
    NoSuitableAsset(String),

    /// Writing the downloaded bytes or extracting a tar entry failed. Also
    /// covers a broken-pipe error on the network side of the
    /// download/extract pipeline, which surfaces here when the *extraction*
    /// half failed first and dropped its end of the pipe.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// The blocking extraction task panicked or was cancelled. Distinct from
    /// [`Io`](ProtonError::Io), which covers errors the extraction *returns*;
    /// this covers it not returning at all.
    #[error("Extraction task failed: {0}")]
    ExtractionError(String),

    /// Creating or canonicalizing the destination directory failed. The
    /// inner `FileSystemError` type is crate-private; only its `Display`
    /// output (via this variant's own message) is visible here.
    #[error("File system error: {0}")]
    FileSystemError(#[from] FileSystemError),

    /// Free space on the destination disk couldn't be determined (e.g. no
    /// mounted disk matches the resolved destination path).
    #[error("Could not resolve free space")]
    CouldNotResolveFreeSpace,

    /// The compressed tarball is larger than the free space on the
    /// destination disk. Note this is a *lower bound* — the extracted tree
    /// is considerably larger than the tarball, so a download that clears
    /// this check can still run the disk out of space mid-extraction and
    /// fail with [`Io`](ProtonError::Io).
    #[error("Not enough free space")]
    NotEnoughFreeSpace,
}
