use serde::Deserialize;

use crate::{
    client::Request,
    games::{GamesError, GamesManager, owned_games::GameId},
};

#[derive(Deserialize, Clone)]
pub struct GameSummary {
    pub summary: Summary,
}

#[derive(Deserialize, Clone)]
pub struct Summary {
    #[serde(alias = "*")]
    pub default: String,
    #[serde(alias = "en-US")]
    pub english: String,
}

impl GameSummary {
    pub async fn get_game_summary(
        games_manager: &GamesManager,
        game_id: GameId,
    ) -> Result<GameSummary, GamesError> {
        {
            let lock = games_manager.inner.lock().await;
            if let Some(game_summary) = lock.game_summary.get(&game_id) {
                return Ok(game_summary.clone());
            }
        }

        let url = format!(
            "https://gamesdb.gog.com/platforms/gog/external_releases/{}",
            game_id
        );

        let game_summary: GameSummary = games_manager.client.fetch(Request::Get { url }).await?;

        let mut lock = games_manager.inner.lock().await;
        lock.game_summary.insert(game_id, game_summary.clone());

        Ok(game_summary)
    }
}
