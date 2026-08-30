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
        let auth = {
            let lock = game_manager.inner.lock().await;
            if let Err(err) = lock.auth.get_auth().await {
                return Err(GamesError::AuthError(err));
            }
            lock.auth.get_auth().await.unwrap()
        };
        let url = "https://embed.gog.com/user/data/games";

        let owned_games: OwnedGames = match game_manager
            .client
            .get_json_with_auth::<OwnedGames>(url, &auth.access_token)
            .await
            .map_err(GamesError::from)
        {
            Ok(owned_games) => owned_games,
            Err(GamesError::AuthError(_err)) => {
                // Token refresh logic
                let auth = {
                    let lock = game_manager.inner.lock().await;
                    if let Err(err) = lock.auth.refresh_auth().await {
                        return Err(GamesError::AuthError(err));
                    }
                    lock.auth.get_auth().await.unwrap()
                };
                match game_manager
                    .client
                    .get_json_with_auth::<OwnedGames>(url, &auth.access_token)
                    .await
                    .map_err(GamesError::from)
                {
                    Ok(owned_games) => owned_games,
                    Err(err) => {
                        return Err(err);
                    }
                }
            }
            Err(err) => {
                return Err(err);
            }
        };
        let mut lock = game_manager.inner.lock().await;
        lock.owned_games = owned_games.clone();
        Ok(owned_games)
    }
}
