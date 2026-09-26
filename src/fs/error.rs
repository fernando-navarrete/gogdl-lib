use std::{io, path::PathBuf};

use thiserror::Error;

/// A failure resolving or touching a path under an install (or save) directory. Arrives wrapped in
/// [`DownloadError::FileSystemError`](crate::DownloadError::FileSystemError),
/// [`ProtonError::FileSystemError`](crate::ProtonError::FileSystemError) and
/// [`SavesError::FileSystemError`](crate::SavesError::FileSystemError).
///
/// `#[non_exhaustive]`: a `match` outside the crate needs a wildcard arm.
#[derive(Error, Debug)]
#[non_exhaustive]
pub enum FileSystemError {
    /// Creating or canonicalizing the base directory failed (for example, permissions).
    #[error("Path resolver creation error: {0}")]
    PathResolverCreationError(io::Error),

    /// Not currently returned by any public method: a metadata failure during a download's
    /// allocation or size checks is reported as a stage event instead. Kept so the enum doesn't
    /// change shape in a minor; slated for removal in `v1.4.0`.
    #[error("File metadata error: {0}")]
    FileMetadataError(io::Error),

    /// Not currently returned by any public method: an allocation failure during a download is
    /// reported as a stage event instead. Kept so the enum doesn't change shape in a minor; slated
    /// for removal in `v1.4.0`.
    #[error("File creation error: {0}")]
    FileCreationError(io::Error),

    /// A relative path could not be resolved under the base directory: it has no file name, it would
    /// escape the base, a component is not a directory, or no ancestor of a queried path exists.
    #[error("Path resolution error: {0}")]
    PathResolutionError(io::Error),

    /// Opening a file for writing a downloaded chunk failed.
    #[error("File open error: {0}")]
    FileOpenError(io::Error),

    /// No mounted disk holds the path, so its free space is unknown. Carries the canonical path.
    #[error("No disk matching path: {0}")]
    NoDiskMatchingPath(PathBuf),
}
