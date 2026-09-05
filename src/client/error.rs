use thiserror::Error;

use crate::client::auth::AuthError;

/// Errors from the underlying HTTP client: transport, response decoding, and
/// the auth layer it wraps around every request.
#[derive(Error, Debug)]
pub enum ClientError {
    /// A URL built from GOG-supplied data (an endpoint template or CDN link)
    /// failed to parse.
    #[error("Url parse error: {0}")]
    UrlParseError(#[from] url::ParseError),

    /// The underlying `reqwest` request failed (connection, TLS, timeout).
    /// Retried automatically inside [`crate::GogDl`]'s methods, up to the
    /// crate's retry budget.
    #[error("Network error: {0}")]
    NetworkError(#[from] reqwest::Error),

    /// The server responded with a non-success status.
    #[error("Http error: status: {status}, body: {body}")]
    HttpError {
        /// The response status code.
        status: reqwest::StatusCode,
        /// The raw response body, for diagnostics.
        body: String,
    },

    /// The response body wasn't valid JSON, or didn't match the expected
    /// shape.
    #[error("Deserialization error: {0}")]
    DeserializationError(#[from] serde_json::Error),

    /// Zlib-decoding a response body failed.
    #[error("Decode error: {0}")]
    DecodeError(std::io::Error),

    /// The caller-supplied streaming callback (used internally by chunk
    /// downloads) returned an I/O error.
    #[error("Chunk stream callback error: {0}")]
    ChunkStreamCallbackError(std::io::Error),

    /// Every retry attempt was exhausted without success or a definitive
    /// failure.
    #[error("Max retires reached")]
    MaxRetriesReached,

    /// The auth layer failed — see [`AuthError`].
    #[error("Auth error: {0}")]
    AuthError(#[from] AuthError),

    /// Resolving a CDN secure link failed while streaming a chunk.
    #[error("Secure links error: {inner}")]
    SecureLinksError {
        /// The stringified underlying
        /// [`SecureLinksError`](crate::SecureLinksError).
        inner: String,
    },
}
