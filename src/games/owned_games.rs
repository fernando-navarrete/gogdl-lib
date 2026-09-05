use std::vec::Vec;

use serde::{Deserialize, Serialize};

use crate::games::{GamesError, GamesManager};

pub type GameId = i32;

/// The product IDs owned by the authenticated account, as returned by
/// [`GogDl::get_owned_games`](crate::GogDl::get_owned_games).
#[derive(Serialize, Deserialize, Clone)]
pub struct OwnedGames {
    /// The owned product IDs.
    pub owned: Vec<GameId>,
}
impl OwnedGames {
    /// An empty `OwnedGames` — the pre-fetch state, never itself returned by
    /// [`GogDl::get_owned_games`](crate::GogDl::get_owned_games).
    pub fn default() -> Self {
        Self { owned: Vec::new() }
    }
    /// Not reachable from outside the crate — `GamesManager` is not
    /// exported. Call
    /// [`GogDl::get_owned_games`](crate::GogDl::get_owned_games) instead,
    /// which delegates here internally.
    pub async fn get_owned_games(game_manager: &GamesManager) -> Result<OwnedGames, GamesError> {
        let owned_games = {
            let lock = game_manager.inner.lock().await;
            lock.owned_games.clone()
        };
        if !owned_games.owned.is_empty() {
            return Ok(owned_games);
        }
        let url = format!("https://embed.gog.com/user/data/games");

        let owned_games: OwnedGames = game_manager.client.fetch(&url, false, true).await?;

        let mut lock = game_manager.inner.lock().await;
        lock.owned_games = owned_games.clone();
        Ok(owned_games)
    }
}
