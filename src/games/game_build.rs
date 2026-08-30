use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::games::{GamesError, GamesManager, owned_games::GameId};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct GameBuild {
    pub build_id: String,
    pub version_name: String,
    pub date_published: DateTime<Utc>,
    pub link: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct GameBuilds {
    #[serde(skip)]
    pub game_title: String,
    pub count: i32,
    pub items: Vec<GameBuild>,
}

impl GameBuilds {
    pub async fn get_game_builds(
        games_manager: &GamesManager,
        game_id: GameId,
    ) -> Result<Self, GamesError> {
        {
            let lock = games_manager.inner.lock().await;
            if let Some(game_details) = lock.game_builds.get(&game_id) {
                return Ok(game_details.clone());
            }
        }
        let auth = {
            let lock = games_manager.inner.lock().await;
            if let Err(err) = lock.auth.get_auth().await {
                return Err(GamesError::AuthError(err));
            }
            lock.auth.get_auth().await.unwrap()
        };
        let url = format!(
            "https://content-system.gog.com/products/{}/os/windows/builds?generation=2",
            game_id
        );

        let game_details: GameBuilds = match games_manager
            .client
            .get_json_with_auth::<GameBuilds>(&url, &auth.access_token)
            .await
            .map_err(GamesError::from)
        {
            Ok(game_details) => game_details,
            Err(GamesError::AuthError(_err)) => {
                // Token refresh logic
                let auth = {
                    let lock = games_manager.inner.lock().await;
                    if let Err(err) = lock.auth.refresh_auth().await {
                        return Err(GamesError::AuthError(err));
                    }
                    lock.auth.get_auth().await.unwrap()
                };
                match games_manager
                    .client
                    .get_json_with_auth::<GameBuilds>(&url, &auth.access_token)
                    .await
                    .map_err(GamesError::from)
                {
                    Ok(game_details) => game_details,
                    Err(err) => {
                        return Err(err);
                    }
                }
            }
            Err(err) => {
                return Err(err);
            }
        };
        let mut lock = games_manager.inner.lock().await;
        lock.game_builds.insert(game_id, game_details.clone());
        Ok(game_details)
    }
}
