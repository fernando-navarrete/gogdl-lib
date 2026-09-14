use serde::Deserialize;

use crate::saves::{error::SavesError, saves_manager::SavesManager};

/// A game-scoped GOG auth grant for the cloud saves service, returned by
/// [`GogDl::get_saves_auth`](crate::GogDl::get_saves_auth).
///
/// Same shape as [`Auth`](crate::Auth), but issued to a specific game's
/// client rather than this crate's session, and **not managed by the
/// crate**: it is never refreshed, stored, or reported to a
/// [`TokenObserver`](crate::TokenObserver). Once `access_token` expires,
/// call [`GogDl::get_saves_auth`](crate::GogDl::get_saves_auth) again.
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
    /// Not populated. GOG's token response has no such field and nothing
    /// computes it after deserialization — always `None`. Derive the expiry
    /// from `expires_in` and the time the call returned instead.
    pub valid_until: Option<i64>,
}

impl SavesAuth {
    /// Not reachable from outside the crate — `SavesManager` is not
    /// exported. Call
    /// [`GogDl::get_saves_auth`](crate::GogDl::get_saves_auth) instead,
    /// which delegates here internally and documents the full contract.
    ///
    /// Holds the manager's `inner` lock only while resolving the build list
    /// and the build metadata, not across the token exchange.
    pub async fn get_saves_auth(
        saves_manager: &SavesManager,
        game_id: i32,
        build_name: &str,
    ) -> Result<Self, SavesError> {
        let game_builds = {
            let inner = saves_manager.inner.lock().await;
            inner.games.get_game_builds(game_id).await?
        };
        let build = game_builds
            .items
            .iter()
            .find(|b| b.version_name == build_name)
            .ok_or(SavesError::BuildNotFound)?;

        let build_metadata = {
            let inner = saves_manager.inner.lock().await;
            inner.depot.get_build_metadata(&build.link).await?
        };
        let client_id = build_metadata.client_id;
        let client_secret = build_metadata.client_secret;

        let refresh_token = saves_manager.client.get_refresh_token().await?;
        let url = format!(
            "https://auth.gog.com/token?client_id={client_id}&client_secret={client_secret}&grant_type=refresh_token&refresh_token={refresh_token}"
        );
        let saves_auth: SavesAuth = saves_manager
            .client
            .fetch_no_retry(&url, false, false, None)
            .await?;

        Ok(saves_auth)
    }
}
