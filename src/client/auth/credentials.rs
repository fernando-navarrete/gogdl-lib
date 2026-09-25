use serde::{Deserialize, Serialize};

use crate::client::auth::AuthError;

/// A GOG auth session: access/refresh tokens plus enough metadata to know
/// when the access token needs refreshing. Persist the output of
/// [`to_string`](Self::to_string) (or the string
/// [`GogDl::login_with_code`](crate::GogDl::login_with_code) returns) and
/// restore it with [`from_string`](Self::from_string) or
/// [`GogDl::restore_auth`](crate::GogDl::restore_auth) on the next run.
///
/// The refresh token rotates on every refresh — the only way to observe that
/// is via a registered
/// [`TokenObserver`](crate::TokenObserver); persisting only the initial
/// login's tokens will eventually restore a dead refresh token.
#[derive(Deserialize, Serialize, Clone)]
pub struct Auth {
    /// Short-lived bearer token sent on every authenticated request.
    pub access_token: String,
    /// Long-lived token used to obtain a new `access_token` once it expires.
    /// Rotates on every refresh.
    pub refresh_token: String,
    /// Access token lifetime in seconds, as reported at issue/refresh time.
    pub expires_in: i32,
    /// Token type as reported by GOG (currently always `"bearer"`).
    pub token_type: String,
    /// GOG session identifier tied to this auth grant.
    pub session_id: String,
    /// OAuth scope string, if GOG returned one.
    pub scope: Option<String>,
    /// The authenticated account's GOG user ID.
    pub user_id: String,
    /// Absolute Unix timestamp the access token expires at, computed at
    /// issue/refresh time as `expires_in` seconds from then. `None` only if
    /// this value was never set — see [`is_valid`](Self::is_valid).
    pub valid_until: Option<i64>,
}

impl Auth {
    /// Serializes this auth state to JSON, for persisting across runs.
    ///
    /// # Errors
    /// [`AuthError::AuthEncodeError`] if serialization fails (should not
    /// happen in practice — every field is a plain, serializable type).
    pub fn to_string(&self) -> Result<String, AuthError> {
        let json_str = match serde_json::to_string(self) {
            Ok(str) => str,
            Err(e) => return Err(AuthError::AuthEncodeError(e)),
        };
        Ok(json_str)
    }
    /// Deserializes an auth state previously produced by
    /// [`to_string`](Self::to_string).
    ///
    /// # Errors
    /// [`AuthError::AuthDecodeError`] if `json_str` isn't a valid serialized
    /// `Auth`.
    pub fn from_string(json_str: &str) -> Result<Auth, AuthError> {
        let tokens: Auth = match serde_json::from_str(json_str) {
            Ok(tokens) => tokens,
            Err(e) => return Err(AuthError::AuthDecodeError(e)),
        };
        Ok(tokens)
    }
    /// Whether the access token is still usable, with a 60-second margin
    /// subtracted from `valid_until` to absorb clock skew and in-flight
    /// requests. `false` if `valid_until` was never set.
    pub fn is_valid(&self) -> bool {
        self.valid_until
            .is_some_and(|t| t > chrono::Utc::now().timestamp() - 60)
    }
}
