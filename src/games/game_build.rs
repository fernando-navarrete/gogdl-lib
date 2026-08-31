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
            if let Some(game_builds) = lock.game_builds.get(&game_id) {
                return Ok(game_builds.clone());
            }
        }
        let url = format!(
            "https://content-system.gog.com/products/{}/os/windows/builds?generation=2",
            game_id
        );

        let game_builds: GameBuilds = games_manager.client.fetch(&url, false, true).await?;
        let mut lock = games_manager.inner.lock().await;
        lock.game_builds.insert(game_id, game_builds.clone());
        Ok(game_builds)
    }
}
