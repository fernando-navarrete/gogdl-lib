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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::with_valid_until;

    fn auth(valid_until: Option<i64>) -> Auth {
        Auth {
            access_token: "access".to_string(),
            refresh_token: "refresh".to_string(),
            expires_in: 3600,
            token_type: "bearer".to_string(),
            session_id: "session".to_string(),
            scope: None,
            user_id: "42".to_string(),
            valid_until,
        }
    }

    // Today's margin has the wrong sign (`t > now - 60`): a token stays
    // "valid" for a minute *after* it expires. `v1.1.2` flips these rows
    // (GAPS "Low — style / clippy"), so `t > now + 60`.
    #[test]
    fn is_valid_at_the_boundaries() {
        for (offset, expected) in [
            (61, true),
            (60, true),
            (59, true),
            (0, true),
            (-59, true),
            (-60, false),
            (-61, false),
        ] {
            assert_eq!(
                with_valid_until(offset, |t| auth(Some(t)).is_valid()),
                expected,
                "valid_until = now {offset:+}s"
            );
        }
    }

    #[test]
    fn is_valid_is_false_without_valid_until() {
        assert!(!auth(None).is_valid());
    }

    #[test]
    fn to_string_and_from_string_round_trip() {
        for (scope, valid_until) in [
            (None, None),
            (Some("scope".to_string()), Some(1_790_000_000)),
        ] {
            let mut original = auth(valid_until);
            original.scope = scope;
            let restored = Auth::from_string(&original.to_string().unwrap()).unwrap();
            assert_eq!(restored.access_token, original.access_token);
            assert_eq!(restored.refresh_token, original.refresh_token);
            assert_eq!(restored.expires_in, original.expires_in);
            assert_eq!(restored.token_type, original.token_type);
            assert_eq!(restored.session_id, original.session_id);
            assert_eq!(restored.scope, original.scope);
            assert_eq!(restored.user_id, original.user_id);
            assert_eq!(restored.valid_until, original.valid_until);
        }
    }

    #[test]
    fn from_string_rejects_what_is_not_an_auth() {
        assert!(matches!(
            Auth::from_string("not json"),
            Err(AuthError::AuthDecodeError(_))
        ));
        assert!(matches!(
            Auth::from_string("{}"),
            Err(AuthError::AuthDecodeError(_))
        ));
    }
}
