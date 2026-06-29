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
        let auth = {
            let lock = games_manager.inner.lock().await;
            if let None = lock.auth.get_auth().await {
                return Err(GamesError::NotAuthenticated);
            }
            lock.auth.get_auth().await.unwrap()
        };
        let url = format!("https://embed.gog.com/account/gameDetails/{}.json", game_id);

        let game_details: GameDetails = match games_manager
            .client
            .get_json_with_auth::<GameDetails>(&url, &auth.access_token)
            .await
            .map_err(GamesError::from)
        {
            Ok(game_details) => game_details,
            Err(GamesError::Unauthorized) => {
                // Token refresh logic
                let auth = {
                    let lock = games_manager.inner.lock().await;
                    if let Err(_err) = lock.auth.refresh_auth().await {
                        return Err(GamesError::Unauthorized);
                    }
                    lock.auth.get_auth().await.unwrap()
                };
                match games_manager
                    .client
                    .get_json_with_auth::<GameDetails>(&url, &auth.access_token)
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
        Ok(game_details)
    }
}
