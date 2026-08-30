use std::vec::Vec;

use serde::{Deserialize, Serialize};

use crate::{
    client::Request,
    games::{GamesError, GamesManager},
};

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
        let auth_manager = {
            let lock = game_manager.inner.lock().await;
            if let Err(err) = lock.auth.get_auth().await {
                return Err(GamesError::AuthError(err));
            }
            lock.auth.clone()
        };
        let url = format!("https://embed.gog.com/user/data/games");

        let owned_games: OwnedGames = game_manager
            .client
            .fetch(Request::GetAuth {
                url: url,
                auth_manager: auth_manager,
            })
            .await?;

        let mut lock = game_manager.inner.lock().await;
        lock.owned_games = owned_games.clone();
        Ok(owned_games)
    }
}
