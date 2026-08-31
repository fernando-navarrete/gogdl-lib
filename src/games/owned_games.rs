use std::vec::Vec;

use serde::{Deserialize, Serialize};

use crate::games::{GamesError, GamesManager};

pub type GameId = i32;

#[derive(Serialize, Deserialize, Clone)]
pub struct OwnedGames {
    pub owned: Vec<GameId>,
}
impl OwnedGames {
    pub fn default() -> Self {
        Self { owned: Vec::new() }
    }
    pub async fn get_owned_games(game_manager: &GamesManager) -> Result<OwnedGames, GamesError> {
        let owned_games = {
            let lock = game_manager.inner.lock().await;
            lock.owned_games.clone()
        };
        if !owned_games.owned.is_empty() {
            return Ok(owned_games);
        }
        let url = format!("https://embed.gog.com/user/data/games");

        let owned_games: OwnedGames = game_manager.client.fetch(&url, true, false).await?;

        let mut lock = game_manager.inner.lock().await;
        lock.owned_games = owned_games.clone();
        Ok(owned_games)
    }
}
