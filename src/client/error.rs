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

    // `{0:?}` (not `{0}`): reqwest's `Display` for body-stream failures
    // collapses to a generic phrase like "error decoding response body",
    // while its `Debug` includes the `kind`, `url`, and the nested `source`
    // error (the actual hyper/h2/io cause) that's needed to tell a real
    // connection reset apart from other causes.
    #[error("Stream error: {0:?}")]
    StreamError(reqwest::Error),

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
            ClientError::StreamError(_) => true,
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
