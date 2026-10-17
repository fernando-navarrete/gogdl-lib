use serde::Deserialize;

use crate::{
    AuthError, ClientError,
    client::{Expiring, Grant, HttpClient},
    constants::TOKEN_URL,
    saves::{error::SavesError, saves_manager::SavesManager},
};

/// A game-scoped GOG auth grant for the cloud saves service.
///
/// Crate-internal: obtained through `SavesManager::get_saves_auth` on
/// behalf of the cloud saves listing, downloads and uploads of
/// [`GogDl`](crate::GogDl).
///
/// Same shape as [`Auth`](crate::Auth), but issued to a specific game's
/// client rather than this crate's session. It is cached in memory and
/// reused while [`is_valid`](Self::is_valid) holds, but never refreshed,
/// persisted, or reported to a [`TokenObserver`](crate::TokenObserver).
/// Once `access_token` expires, the next call performs a fresh exchange.
#[derive(Deserialize, Clone)]
pub struct SavesAuth {
    /// Short-lived bearer token sent on every authenticated request.
    pub access_token: String,
    /// Access token lifetime in seconds, as reported at issue/refresh time.
    pub expires_in: i32,
    /// The authenticated account's GOG user ID.
    pub user_id: String,
    /// Unix timestamp (seconds) at which `access_token` expires, computed
    /// from `expires_in` when the grant is issued. GOG's response has no
    /// such field, so it is `None` only for a value deserialized elsewhere.
    pub valid_until: Option<i64>,
}

impl SavesAuth {
    /// Called through `SavesManager::get_saves_auth`; the
    /// public-facing contract of the grant (caching, expiry, no refresh) is
    /// documented on [`GogDl::get_save_files`](crate::GogDl::get_save_files).
    ///
    /// Returns the cached grant for `(game_id, build_name)` if it is still
    /// [valid](Self::is_valid); otherwise resolves the game's credentials via
    /// `SavesManager::get_game_save_ids`, performs the exchange, and caches
    /// the result. Holds the manager's `inner` lock only for the cache lookup
    /// and insert, not across the token exchange.
    pub(crate) async fn get_saves_auth(
        saves_manager: &SavesManager,
        game_id: i32,
        build_name: &str,
    ) -> Result<Self, SavesError> {
        {
            let inner = saves_manager.inner.lock().await;
            if let Some(auth) = inner.auth_cache.get(&(game_id, build_name.to_string()))
                && auth.is_valid()
            {
                return Ok(auth.clone());
            }
        }
        let game_ids = saves_manager.get_game_save_ids(game_id, build_name).await?;

        let client_id = game_ids.client_id;
        let client_secret = game_ids.client_secret;
        let refresh_token = saves_manager.client.get_refresh_token().await?;
        let mut saves_auth = Self::exchange(
            &saves_manager.client,
            TOKEN_URL,
            &client_id,
            &client_secret,
            &refresh_token,
        )
        .await?;
        saves_auth.valid_until =
            Some(saves_auth.expires_in as i64 + chrono::Utc::now().timestamp());

        {
            let mut inner = saves_manager.inner.lock().await;
            inner
                .auth_cache
                .insert((game_id, build_name.to_string()), saves_auth.clone());
        }
        Ok(saves_auth)
    }
    /// POSTs the game-scoped refresh grant to `url`. The grant goes in a form
    /// body and the transport error has its URL stripped, so neither the
    /// session's refresh token nor the game's `client_secret` can reach an
    /// error string. A failure is classified like the session's own refresh
    /// (see [`AuthError::from_token_failure`]), so a dead session is
    /// [`AuthError::SessionRevoked`].
    async fn exchange(
        client: &HttpClient,
        url: &str,
        client_id: &str,
        client_secret: &str,
        refresh_token: &str,
    ) -> Result<Self, SavesError> {
        Ok(client
            .post_token(
                url,
                &[
                    ("client_id", client_id),
                    ("client_secret", client_secret),
                    ("grant_type", "refresh_token"),
                    ("refresh_token", refresh_token),
                ],
            )
            .await
            .map_err(|failure| {
                ClientError::from(AuthError::from_token_failure(Grant::RefreshToken, failure))
            })?)
    }
    /// The URL under `base` (in production [`CLOUD_STORAGE_URL`]) of the
    /// object called `name` in this user's storage area for the game
    /// `client_id`. `name` is the full cloud-side name, e.g.
    /// `saves/AutoSave-0/sav.dat`; each `/`-separated segment is
    /// percent-encoded, so names with spaces or `#` are safe.
    pub fn object_url(
        &self,
        base: &str,
        client_id: &str,
        name: &str,
    ) -> Result<reqwest::Url, SavesError> {
        let mut url = reqwest::Url::parse(base).map_err(ClientError::UrlParseError)?;
        url.path_segments_mut()
            .map_err(|_| {
                ClientError::UrlParseError(url::ParseError::RelativeUrlWithCannotBeABaseBase)
            })?
            .pop_if_empty()
            .push(&self.user_id)
            .push(client_id)
            .extend(name.split('/'));
        Ok(url)
    }
    /// Whether the access token is still usable: `valid_until` must be more
    /// than 60 seconds away, so a token is treated as expired a minute early
    /// to absorb clock skew and in-flight requests. `false` if `valid_until`
    /// was never set.
    pub fn is_valid(&self) -> bool {
        self.is_fresh_at(chrono::Utc::now().timestamp())
    }
}

impl Expiring for SavesAuth {
    const FRESH_WITHOUT_DEADLINE: bool = false;

    fn deadline(&self) -> Option<i64> {
        self.valid_until
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{constants::CLOUD_STORAGE_URL, test_support::with_valid_until};

    fn auth() -> SavesAuth {
        SavesAuth {
            access_token: String::new(),
            expires_in: 0,
            user_id: "42".to_string(),
            valid_until: None,
        }
    }

    #[tokio::test]
    async fn a_failed_exchange_leaks_neither_the_refresh_token_nor_the_client_secret() {
        use crate::test_support::{ChunkServer, Reply};
        let server = ChunkServer::start().await;
        for (reply, network) in [(Reply::Close, true), (Reply::Status(400), false)] {
            server.script("/token", vec![reply]);
            let err = SavesAuth::exchange(
                &HttpClient::new_with_client(reqwest::Client::new()),
                &format!("{}/token", server.base_url()),
                "client",
                "SECRET-SENTINEL",
                "REFRESH-SENTINEL",
            )
            .await
            .err()
            .unwrap();
            let SavesError::ClientError(ClientError::AuthError(kind)) = &err else {
                panic!("expected an AuthError, got {err:?}");
            };
            assert_eq!(
                matches!(kind, AuthError::NetworkError(_)),
                network,
                "{err:?}"
            );
            assert_eq!(
                matches!(kind, AuthError::Rejected { .. }),
                !network,
                "{err:?}"
            );
            for text in [err.to_string(), format!("{err:?}")] {
                assert!(text.contains("rror"), "not an error string: {text}");
                assert!(!text.contains("SECRET-SENTINEL"), "leaked: {text}");
                assert!(!text.contains("REFRESH-SENTINEL"), "leaked: {text}");
            }
        }
    }

    // Captured from the real endpoint (v1.4.0 plan, step 1): a dead refresh token
    // is `400 invalid_grant` for a game's client too.
    #[tokio::test]
    async fn a_dead_session_is_session_revoked() {
        use crate::test_support::{ChunkServer, Reply};
        let body = include_str!("../../tests/fixtures/token_error_saves_refresh_garbage.json");
        let expected =
            serde_json::from_str::<serde_json::Value>(body).unwrap()["error_description"]
                .as_str()
                .unwrap()
                .to_string();
        let server = ChunkServer::start().await;
        server.script(
            "/token",
            vec![Reply::StatusBody(400, body.as_bytes().to_vec())],
        );
        let err = SavesAuth::exchange(
            &HttpClient::new_with_client(reqwest::Client::new()),
            &format!("{}/token", server.base_url()),
            "client",
            "SECRET-SENTINEL",
            "REFRESH-SENTINEL",
        )
        .await
        .err()
        .unwrap();
        let SavesError::ClientError(ClientError::AuthError(AuthError::SessionRevoked {
            description,
        })) = &err
        else {
            panic!("expected SessionRevoked, got {err:?}");
        };
        assert_eq!(description, &expected);
        for text in [err.to_string(), format!("{err:?}")] {
            assert!(!text.contains("SECRET-SENTINEL"), "leaked: {text}");
            assert!(!text.contains("REFRESH-SENTINEL"), "leaked: {text}");
        }
    }

    #[test]
    fn object_url_addresses_the_object_in_the_users_storage_area() {
        let url = auth()
            .object_url(
                CLOUD_STORAGE_URL,
                "client",
                "saves/__default/profile/slot1.sav",
            )
            .unwrap();
        assert_eq!(
            url.as_str(),
            "https://cloudstorage.gog.com/v1/42/client/saves/__default/profile/slot1.sav"
        );
    }

    #[test]
    fn object_url_escapes_characters_that_would_end_the_path() {
        let url = auth()
            .object_url(
                CLOUD_STORAGE_URL,
                "client",
                "saves/__default/my save #1?.sav",
            )
            .unwrap();
        assert_eq!(
            url.path(),
            "/v1/42/client/saves/__default/my%20save%20%231%3F.sav"
        );
        assert_eq!(url.query(), None);
        assert_eq!(url.fragment(), None);
    }

    // Same margin as `Auth::is_valid`: more than 60s before `valid_until`.
    #[test]
    fn is_valid_at_the_boundaries() {
        for (offset, expected) in [
            (61, true),
            (60, false),
            (59, false),
            (0, false),
            (-59, false),
            (-60, false),
            (-61, false),
        ] {
            let valid = with_valid_until(offset, |t| {
                SavesAuth {
                    valid_until: Some(t),
                    ..auth()
                }
                .is_valid()
            });
            assert_eq!(valid, expected, "valid_until = now {offset:+}s");
        }
    }

    #[test]
    fn is_valid_is_false_without_valid_until() {
        assert!(!auth().is_valid());
    }
}
