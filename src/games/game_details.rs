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

/// What `embed.gog.com/account/gameDetails/{id}.json` answers: an object for a
/// game, and an empty array (still a 200) for a DLC or a pack.
#[derive(Deserialize)]
#[serde(untagged)]
enum DetailsResponse {
    Game(GameDetails),
    NotAGame(#[allow(dead_code)] Vec<serde::de::IgnoredAny>),
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

        let url = games_manager.game_details_url(game_id);

        let response: DetailsResponse = games_manager.client.fetch(&url, false, true, None).await?;
        let game_details = match response {
            DetailsResponse::Game(game_details) => Some(game_details),
            DetailsResponse::NotAGame(_) => None,
        };

        let mut lock = games_manager.inner.lock().await;
        lock.game_details.insert(game_id, game_details.clone());

        game_details.ok_or(GamesError::ProductNotAGame)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Captured from `embed.gog.com/account/gameDetails/{id}.json` for a DLC
    // and a pack the account owns: 200 with an empty JSON array, not an
    // object, which `get_game_details` maps to `ProductNotAGame`.
    #[test]
    fn a_non_games_details_are_an_empty_array() {
        let body = include_str!("../../tests/fixtures/game_details_non_game.json");
        assert!(serde_json::from_str::<GameDetails>(body).is_err());
        assert!(matches!(
            serde_json::from_str::<DetailsResponse>(body).unwrap(),
            DetailsResponse::NotAGame(_)
        ));
    }

    #[test]
    fn a_games_details_decode() {
        let body = include_str!("../../tests/fixtures/game_details_game.json");
        let details: GameDetails = serde_json::from_str(body).unwrap();
        assert_eq!(details.title, "Cyberpunk 2077");
    }
}
