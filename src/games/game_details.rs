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

        let auth = {
            let lock = games_manager.inner.lock().await;
            lock.auth.clone()
        };
        let url = format!("https://embed.gog.com/account/gameDetails/{}.json", game_id);

        let game_details: GameDetails = match auth
            .authorized_get_json(&games_manager.client, &url)
            .await
        {
            Ok(game_details) => game_details,
            Err(err) => {
                let err = GamesError::from(err);
                // A non-game product's response body doesn't match
                // `GameDetails`'s shape, which surfaces as a decode error —
                // negative-cache that as "not a game" so we don't re-fetch
                // and re-fail on every subsequent lookup.
                if let GamesError::DecodeError(_) = err {
                    let mut lock = games_manager.inner.lock().await;
                    lock.game_details.insert(game_id, None);
                    return Err(GamesError::ProductNotAGame);
                }
                return Err(err);
            }
        };

        let mut lock = games_manager.inner.lock().await;
        lock.game_details
            .insert(game_id, Some(game_details.clone()));

        Ok(game_details)
    }
}
