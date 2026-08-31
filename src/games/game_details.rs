use serde::{Deserialize, Serialize};

use crate::games::{GamesError, GamesManager, owned_games::GameId};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct GameDetails {
    pub title: String,
    #[serde(skip)]
    pub id: GameId,
}

impl GameDetails {
    pub async fn get_game_details(
        games_manager: &GamesManager,
        game_id: GameId,
    ) -> Result<GameDetails, GamesError> {
        {
            let lock = games_manager.inner.lock().await;
            if let Some(game_details) = lock.game_details.get(&game_id) {
                match game_details {
                    Some(game_details) => return Ok(game_details.clone()),
                    None => return Err(GamesError::ProductNotAGame),
                }
            }
        }

        let url = format!("https://embed.gog.com/account/gameDetails/{}.json", game_id);

        let game_details: GameDetails = games_manager.client.fetch(&url, false, true).await?;

        let mut lock = games_manager.inner.lock().await;
        lock.game_details
            .insert(game_id, Some(game_details.clone()));

        Ok(game_details)
    }
}
