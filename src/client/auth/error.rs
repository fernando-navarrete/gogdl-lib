use thiserror::Error;

/// Errors from the auth layer: local token state and the login/refresh
/// round-trip to GOG.
#[derive(Error, Debug)]
pub enum AuthError {
    /// No [`Auth`](crate::Auth) has been established yet — call
    /// [`GogDl::login_with_code`](crate::GogDl::login_with_code) or
    /// [`GogDl::restore_auth`](crate::GogDl::restore_auth) first.
    #[error("Not yet authenticated")]
    NotAuthenticated,

    /// The locally-held access token is past its computed expiry. Callers
    /// that go through [`crate::GogDl`]'s methods never see this directly —
    /// it triggers an automatic refresh-and-retry instead.
    #[error("Auth token expired locally")]
    TokenExpired,

    /// [`Auth::from_string`](crate::Auth::from_string) was given data that
    /// doesn't deserialize.
    #[error("Could not decode local auth token: {0}")]
    AuthDecodeError(serde_json::Error),

    /// [`Auth::to_string`](crate::Auth::to_string) failed to serialize.
    #[error("Could not encode token: {0}")]
    AuthEncodeError(serde_json::Error),

    /// The login or refresh HTTP call itself failed (network, non-200, or
    /// undecodable body). `inner` is a `Display`-formatted
    /// [`ClientError`](crate::ClientError) with no further structure — a
    /// dead refresh token and a transient network blip both land here today.
    #[error("Client error: {inner}")]
    ClientError {
        /// The stringified underlying [`ClientError`](crate::ClientError).
        inner: String,
    },
}
