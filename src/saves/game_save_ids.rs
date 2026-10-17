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
    /// Called through `SavesManager::get_game_save_ids`.
    ///
    /// Returns the credentials for `(game_id, build_name)` from the
    /// manager's `ids_cache` if present; otherwise finds the build whose
    /// [`version_name`](crate::GameBuild::version_name) equals `build_name`,
    /// reads `client_id`/`client_secret` from its build metadata, and caches
    /// them. Cache entries never expire.
    ///
    /// Holds the manager's `inner` lock only for the cache lookup and insert,
    /// never across a fetch, so concurrent calls for an uncached key may each
    /// fetch the metadata. The metadata itself is cached by `DepotManager`, so a miss here is
    /// cheap once the builds list is cached.
    ///
    /// # Errors
    /// - [`SavesError::BuildNotFound`] if no build of `game_id` has a
    ///   matching `version_name`.
    /// - [`SavesError::GamesError`] if listing the game's builds fails.
    /// - [`SavesError::DepotError`] if fetching the build metadata fails.
    pub(crate) async fn get_game_save_ids(
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
        let game_builds = saves_manager.games.get_game_builds(game_id).await?;
        let build = game_builds
            .items
            .iter()
            .find(|b| b.version_name == build_name)
            .ok_or(SavesError::BuildNotFound)?;

        let build_metadata = saves_manager.depot.get_build_metadata(&build.link).await?;
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

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Duration};

    use super::*;
    use crate::{client::HttpClient, depot::DepotManager, games::GamesManager};

    /// A cold lookup stuck inside the games manager (its `inner` is held here,
    /// the await the manager's own lock used to span) must not block a cached
    /// lookup for another game.
    #[tokio::test(start_paused = true)]
    async fn a_cold_lookup_does_not_block_a_cached_one() {
        let client = HttpClient::new_with_client(reqwest::Client::new());
        let games = GamesManager::new(client.clone());
        let saves = Arc::new(SavesManager::new(
            client.clone(),
            DepotManager::new(client),
            games.clone(),
        ));
        saves.inner.lock().await.ids_cache.insert(
            (2, "b".to_string()),
            GameSaveIds {
                client_id: "id".into(),
                client_secret: "secret".into(),
            },
        );

        let games_guard = games.inner.lock().await;
        let cold = tokio::spawn({
            let saves = saves.clone();
            async move { saves.get_game_save_ids(1, "a").await }
        });
        tokio::time::sleep(Duration::from_millis(1)).await;

        let cached = tokio::time::timeout(Duration::from_secs(1), saves.get_game_save_ids(2, "b"))
            .await
            .expect("a cached lookup waited on another game's fetch")
            .unwrap();
        assert_eq!(cached.client_id, "id");

        drop(games_guard);
        cold.abort();
    }
}
