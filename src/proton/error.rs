use std::path::PathBuf;

use thiserror::Error;

use crate::ClientError;
use crate::fs::{FileSystemError, FreeSpaceShortfall};

/// Errors from listing ([`GogDl::get_proton_releases`](crate::GogDl::get_proton_releases)),
/// fetching one ([`GogDl::get_proton_release_by_tag`](crate::GogDl::get_proton_release_by_tag))
/// or downloading ([`GogDl::download_proton_release`](crate::GogDl::download_proton_release))
/// Proton-GE releases.
///
/// Listing releases has no separate decode/auth path of its own, so any
/// failure there (network, a non-2xx status, or a JSON decode error) arrives
/// as [`ClientError`]. Downloading a release can additionally fail with any
/// of the other variants here.
///
/// `#[non_exhaustive]`: a `match` outside the crate needs a wildcard arm.
#[non_exhaustive]
#[derive(Debug, Error)]
pub enum ProtonError {
    /// The underlying HTTP request to GitHub failed — see [`ClientError`].
    /// Notably includes a 403 from an exhausted rate limit; see
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
    /// cause is a [`FileSystemError`](crate::FileSystemError).
    #[error("File system error: {0}")]
    FileSystemError(#[from] FileSystemError),

    /// Free space on the destination disk couldn't be determined (e.g. no
    /// mounted disk matches the resolved destination path).
    #[error("Could not resolve free space for {}", path.display())]
    CouldNotResolveFreeSpace {
        /// The resolved destination path no disk matched.
        path: PathBuf,
    },

    /// The compressed tarball is larger than the free space on the
    /// destination disk. Note this is a *lower bound* — the extracted tree
    /// is considerably larger than the tarball, so a download that clears
    /// this check can still run the disk out of space mid-extraction and
    /// fail with [`Io`](ProtonError::Io), which leaves the destination as it
    /// was. A re-download keeps the old install until the new one is ready.
    #[error("Not enough free space: {required} bytes required, {available} available")]
    NotEnoughFreeSpace {
        /// The compressed tarball's size in bytes.
        required: u64,
        /// Bytes available on the destination disk.
        available: u64,
    },
}

impl From<FreeSpaceShortfall> for ProtonError {
    fn from(shortfall: FreeSpaceShortfall) -> Self {
        match shortfall {
            FreeSpaceShortfall::Unresolved(path) => Self::CouldNotResolveFreeSpace { path },
            FreeSpaceShortfall::NotEnough {
                required,
                available,
            } => Self::NotEnoughFreeSpace {
                required,
                available,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shortfall_keeps_its_numbers_and_path() {
        let err = ProtonError::from(FreeSpaceShortfall::NotEnough {
            required: 10,
            available: 3,
        });
        assert!(matches!(
            err,
            ProtonError::NotEnoughFreeSpace {
                required: 10,
                available: 3
            }
        ));
        let err = ProtonError::from(FreeSpaceShortfall::Unresolved(PathBuf::from("/x")));
        assert!(matches!(
            err,
            ProtonError::CouldNotResolveFreeSpace { ref path } if path == "/x"
        ));
    }
}
