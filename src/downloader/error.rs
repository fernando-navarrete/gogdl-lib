use std::io;

use reqwest::StatusCode;
use thiserror::Error;

use crate::{
    client::ClientError, depot::DepotError, downloader::fs::FileSystemError, games::GamesError,
    secure_links::SecureLinksError,
};

#[derive(Error, Debug)]
pub enum DownloadError {
    #[error("Url parse error: {0}")]
    UrlParseError(#[from] url::ParseError),

    #[error("Network error: {0}")]
    NetworkError(#[from] reqwest::Error),

    #[error("Http error: {body}, status: {status}")]
    Http { status: StatusCode, body: String },

    #[error("Decode error: {0}")]
    DecodeError(#[from] serde_json::Error),

    #[error("Deflate error: {0}")]
    DeflateError(io::Error),

    #[error("Game error: {0}")]
    GamesError(#[from] GamesError),

    #[error("Depot error: {0}")]
    DepotError(#[from] DepotError),

    #[error("Build not found")]
    BuildNotFound,

    #[error("File allocation error")]
    FileAllocationError,

    #[error("Secure links error: {0}")]
    SecureLinksError(#[from] SecureLinksError),

    #[error("File system error: {0}")]
    FileSystemError(#[from] FileSystemError),

    #[error("Client error: {0}")]
    ClientError(#[from] ClientError),

    #[error("Chunk integrity check failed: incomplete chunks: {0}")]
    ChunkIntegrityCheckFailed(usize),
}
