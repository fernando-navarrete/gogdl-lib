use crate::{SavesError, saves::SavesManager};

/// A game's own OAuth client credentials, taken from the build metadata of
/// one of its builds. Cloud saves are scoped to these rather than to this
/// crate's session client.
///
/// Crate-internal: resolved and cached by `SavesManager` on behalf of
/// the cloud saves methods of [`GogDl`](crate::GogDl).
#[derive(Clone)]
pub struct GameSaveIds {
    /// The game's OAuth client ID. Also identifies the game's storage area
    /// under `cloudstorage.gog.com`.
    pub client_id: String,
    /// The game's OAuth client secret, used to exchange the session's
    /// refresh token for a game-scoped `SavesAuth`.
    pub client_secret: String,
}

impl GameSaveIds {
    /// Not reachable from outside the crate — `SavesManager` is not
    /// exported. Called through `SavesManager::get_game_save_ids`.
    ///
    /// Returns the credentials for `(game_id, build_name)` from the
    /// manager's `ids_cache` if present; otherwise finds the build whose
    /// [`version_name`](crate::GameBuild::version_name) equals `build_name`,
    /// reads `client_id`/`client_secret` from its build metadata, and caches
    /// them. Cache entries never expire.
    ///
    /// Holds the manager's `inner` lock for the cache lookup, while listing
    /// builds, while fetching build metadata, and for the cache insert — but
    /// releases it in between, so concurrent calls for an uncached key may
    /// each fetch the metadata.
    ///
    /// # Errors
    /// - [`SavesError::BuildNotFound`] if no build of `game_id` has a
    ///   matching `version_name`.
    /// - [`SavesError::GamesError`] if listing the game's builds fails.
    /// - [`SavesError::DepotError`] if fetching the build metadata fails.
    pub async fn get_game_save_ids(
        saves_manager: &SavesManager,
        game_id: i32,
        build_name: &str,
    ) -> Result<Self, SavesError> {
        {
            let inner = saves_manager.inner.lock().await;
            if let Some(ids) = inner.ids_cache.get(&(game_id, build_name.to_string())) {
                return Ok(ids.clone());
            }
        }
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
        let game_ids = GameSaveIds {
            client_id,
            client_secret,
        };
        {
            let mut inner = saves_manager.inner.lock().await;
            inner
                .ids_cache
                .insert((game_id, build_name.to_string()), game_ids.clone());
        }
        Ok(game_ids)
    }
}
