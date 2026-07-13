use std::io;

use reqwest::StatusCode;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ClientError {
    #[error("Url parse error: {0}")]
    UrlParseError(#[from] url::ParseError),

    #[error("Network error: {0}")]
    NetworkError(#[from] reqwest::Error),

    #[error("Http error: {body}, status: {status}")]
    Http { status: StatusCode, body: String },

    #[error("Decode error: {0}")]
    DecodeError(#[from] serde_json::Error),

    #[error("Deflate error: {0}")]
    DeflateError(#[from] io::Error),

    #[error("Stream error")]
    StreamError(),

    #[error("Request timed out")]
    Timeout,
}

impl ClientError {
    /// Whether retrying the same request again is worth attempting: stalls,
    /// resets, and server-side/rate-limit responses usually clear up on
    /// their own, while a bad URL or an auth/not-found response won't.
    pub fn is_transient(&self) -> bool {
        match self {
            ClientError::Timeout => true,
            ClientError::StreamError() => true,
            ClientError::NetworkError(err) => {
                err.is_timeout() || err.is_connect() || err.is_request() || err.is_body()
            }
            ClientError::Http { status, .. } => {
                status.is_server_error() || *status == StatusCode::TOO_MANY_REQUESTS
            }
            ClientError::UrlParseError(_)
            | ClientError::DecodeError(_)
            | ClientError::DeflateError(_) => false,
        }
    }
}
