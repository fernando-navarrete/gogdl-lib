use std::{io, path::PathBuf};

use thiserror::Error;

#[derive(Error, Debug)]
pub enum FileSystemError {
    #[error("Path resolver creation error: {0}")]
    PathResolverCreationError(io::Error),

    #[error("File metadata error: {0}")]
    FileMetadataError(io::Error),

    #[error("File creation error: {0}")]
    FileCreationError(io::Error),

    #[error("Path resolution error: {0}")]
    PathResolutionError(io::Error),

    #[error("File open error: {0}")]
    FileOpenError(io::Error),

    #[error("No disk matching path: {0}")]
    NoDiskMatchingPath(PathBuf),
}
