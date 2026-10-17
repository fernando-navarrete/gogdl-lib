use thiserror::Error;

/// Errors from the auth layer: local token state and the login/refresh
/// round-trip to GOG.
///
/// `#[non_exhaustive]`: a `match` outside the crate needs a wildcard arm.
#[derive(Error, Debug)]
#[non_exhaustive]
pub enum AuthError {
    /// No [`Auth`](crate::Auth) has been established yet — call
    /// [`GogDl::login_with_code`](crate::GogDl::login_with_code) or
    /// [`GogDl::restore_auth`](crate::GogDl::restore_auth) first.
    #[error("Not yet authenticated")]
    NotAuthenticated,

    /// The locally-held access token is past its computed expiry. Callers
    /// that go through [`crate::GogDl`]'s methods never see this directly —
    /// it triggers an automatic refresh-and-retry instead. The exception is
    /// the cloud saves methods, which need a still-valid access token and
    /// return this (wrapped in [`SavesError::ClientError`](crate::SavesError))
    /// rather than refresh.
    #[error("Auth token expired locally")]
    TokenExpired,

    /// [`Auth::from_string`](crate::Auth::from_string) was given data that
    /// doesn't deserialize.
    #[error("Could not decode local auth token: {0}")]
    AuthDecodeError(serde_json::Error),

    /// [`Auth::to_string`](crate::Auth::to_string) failed to serialize.
    #[error("Could not encode token: {0}")]
    AuthEncodeError(serde_json::Error),

    /// GOG rejected the authorization code given to
    /// [`GogDl::login_with_code`](crate::GogDl::login_with_code): `400
    /// invalid_grant` on the `authorization_code` grant. A mistyped or
    /// truncated code and an expired one are the same kind; GOG tells them
    /// apart only in `description` ("Code doesn't exist or is invalid for the
    /// client", "The authorization code has expired").
    #[error("Login code rejected: {description}")]
    InvalidLoginCode {
        /// GOG's `error_description`, empty if the reply had none.
        description: String,
    },

    /// GOG rejected the refresh token: `400 invalid_grant` on the
    /// `refresh_token` grant ("Invalid refresh token, client_id:…"). The
    /// session is dead and the user has to log in again. It can come out of
    /// any authenticated method, since the refresh happens inside the request.
    #[error("Session revoked: {description}")]
    SessionRevoked {
        /// GOG's `error_description`, empty if the reply had none. It names
        /// the client id, never a token.
        description: String,
    },

    /// The login or refresh request never got a reply (connection, TLS,
    /// timeout). The session may well be fine; try again. The request URL is
    /// stripped from the error.
    #[error("Network error: {0}")]
    NetworkError(reqwest::Error),

    /// GOG answered the login or refresh request with a non-success status
    /// that is not `invalid_grant` (a 5xx, say, or an unrecognized body).
    #[error("Token request rejected: status: {status}, body: {body}")]
    Rejected {
        /// The response status code.
        status: reqwest::StatusCode,
        /// The raw response body, for diagnostics.
        body: String,
    },

    /// GOG answered `200` but the body isn't a valid [`Auth`](crate::Auth).
    #[error("Invalid token response: {0}")]
    InvalidResponse(serde_json::Error),
}

/// The OAuth grant a token POST carries; it decides which kind an
/// `invalid_grant` reply becomes.
#[derive(Clone, Copy)]
pub(crate) enum Grant {
    AuthorizationCode,
    RefreshToken,
}

impl Grant {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Grant::AuthorizationCode => "authorization_code",
            Grant::RefreshToken => "refresh_token",
        }
    }
}

/// How a token POST failed, before it is classified into an [`AuthError`].
pub(crate) enum TokenFailure {
    Network(reqwest::Error),
    Status {
        status: reqwest::StatusCode,
        body: String,
    },
    Decode(serde_json::Error),
}

#[derive(serde::Deserialize)]
struct TokenErrorBody {
    error: String,
    #[serde(default)]
    error_description: String,
}

impl AuthError {
    /// The one place a failed token POST becomes an [`AuthError`]. The grant,
    /// not the body, picks between a rejected login code and a revoked
    /// session: GOG answers `400 invalid_grant` for both.
    pub(crate) fn from_token_failure(grant: Grant, failure: TokenFailure) -> Self {
        match failure {
            TokenFailure::Network(e) => AuthError::NetworkError(e),
            TokenFailure::Decode(e) => AuthError::InvalidResponse(e),
            TokenFailure::Status { status, body } => {
                match serde_json::from_str::<TokenErrorBody>(&body) {
                    Ok(b) if b.error == "invalid_grant" => match grant {
                        Grant::AuthorizationCode => AuthError::InvalidLoginCode {
                            description: b.error_description,
                        },
                        Grant::RefreshToken => AuthError::SessionRevoked {
                            description: b.error_description,
                        },
                    },
                    _ => AuthError::Rejected { status, body },
                }
            }
        }
    }
}

impl From<TokenFailure> for crate::ClientError {
    fn from(failure: TokenFailure) -> Self {
        match failure {
            TokenFailure::Network(e) => Self::NetworkError(e),
            TokenFailure::Status { status, body } => Self::HttpError { status, body },
            TokenFailure::Decode(e) => Self::DeserializationError(e),
        }
    }
}
