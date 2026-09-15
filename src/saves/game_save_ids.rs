use crate::{SavesError, saves::SavesManager};

#[derive(Clone)]
pub struct GameSaveIds {
    pub client_id: String,
    pub client_secret: String,
}

impl GameSaveIds {
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
