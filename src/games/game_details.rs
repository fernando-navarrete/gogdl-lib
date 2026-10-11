use serde::{Deserialize, Serialize};

use crate::games::{GamesError, GamesManager, owned_products::ProductId};

/// Details for a single game, as returned by
/// [`GogDl::get_game_details`](crate::GogDl::get_game_details).
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct GameDetails {
    /// The game's display title.
    pub title: String,
    /// Not populated. `#[serde(skip)]` and never assigned after
    /// deserialization — always `0`. Use the `game_id` you already passed to
    /// `get_game_details` instead of reading this back.
    #[serde(skip)]
    pub id: ProductId,
}

impl GameDetails {
    /// Not reachable from outside the crate — `GamesManager` is not
    /// exported. Call
    /// [`GogDl::get_game_details`](crate::GogDl::get_game_details) instead,
    /// which delegates here internally.
    pub async fn get_game_details(
        games_manager: &GamesManager,
        game_id: ProductId,
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

        let game_details: GameDetails = games_manager.client.fetch(&url, false, true, None).await?;

        let mut lock = games_manager.inner.lock().await;
        lock.game_details
            .insert(game_id, Some(game_details.clone()));

        Ok(game_details)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Captured from `embed.gog.com/account/gameDetails/{id}.json` for a DLC
    // and a pack the account owns: 200 with an empty JSON array, not an
    // object. Step 10 maps this to `ProductNotAGame`.
    #[test]
    fn a_non_games_details_are_an_empty_array_that_fails_to_decode() {
        let body = include_str!("../../tests/fixtures/game_details_non_game.json");
        assert!(serde_json::from_str::<GameDetails>(body).is_err());
    }

    #[test]
    fn a_games_details_decode() {
        let body = include_str!("../../tests/fixtures/game_details_game.json");
        let details: GameDetails = serde_json::from_str(body).unwrap();
        assert_eq!(details.title, "Cyberpunk 2077");
    }
}
