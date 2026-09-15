use serde::Deserialize;

use crate::saves::{error::SavesError, saves_manager::SavesManager};

/// A game-scoped GOG auth grant for the cloud saves service.
///
/// Crate-internal: obtained through `SavesManager::get_saves_auth` on
/// behalf of [`GogDl::get_save_files`](crate::GogDl::get_save_files).
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
    /// Unix timestamp (seconds) at which `access_token` expires, computed
    /// from `expires_in` when the grant is issued. GOG's response has no
    /// such field, so it is `None` only for a value deserialized elsewhere.
    pub valid_until: Option<i64>,
}

impl SavesAuth {
    /// Not reachable from outside the crate — `SavesManager` is not
    /// exported. Called through `SavesManager::get_saves_auth`; the
    /// public-facing contract is documented on
    /// [`GogDl::get_save_files`](crate::GogDl::get_save_files).
    ///
    /// Returns the cached grant for `(game_id, build_name)` if it is still
    /// [valid](Self::is_valid); otherwise resolves the game's credentials via
    /// `SavesManager::get_game_save_ids`, performs the exchange, and caches
    /// the result. Holds the manager's `inner` lock only for the cache lookup
    /// and insert, not across the token exchange.
    pub async fn get_saves_auth(
        saves_manager: &SavesManager,
        game_id: i32,
        build_name: &str,
    ) -> Result<Self, SavesError> {
        {
            let inner = saves_manager.inner.lock().await;
            if let Some(auth) = inner.auth_cache.get(&(game_id, build_name.to_string())) {
                if auth.is_valid() {
                    return Ok(auth.clone());
                }
            }
        }
        let game_ids = saves_manager.get_game_save_ids(game_id, build_name).await?;

        let client_id = game_ids.client_id;
        let client_secret = game_ids.client_secret;
        let refresh_token = saves_manager.client.get_refresh_token().await?;
        let url = format!(
            "https://auth.gog.com/token?client_id={client_id}&client_secret={client_secret}&grant_type=refresh_token&refresh_token={refresh_token}"
        );
        let mut saves_auth: SavesAuth = saves_manager
            .client
            .fetch_no_retry(&url, false, false, None)
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
    /// Whether the access token is still usable, with a 60-second margin
    /// subtracted from `valid_until` to absorb clock skew and in-flight
    /// requests. `false` if `valid_until` was never set.
    pub fn is_valid(&self) -> bool {
        self.valid_until
            .map_or(false, |t| t > chrono::Utc::now().timestamp() - 60)
    }
}
