use serde::Deserialize;

use crate::games::{GamesError, GamesManager, owned_products::ProductId};

/// The store summary/description text for a game, as returned by
/// [`GogDl::get_game_summary`](crate::GogDl::get_game_summary).
#[derive(Deserialize, Clone)]
pub struct GameSummary {
    /// The summary text, in the default and English locales.
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
    /// Not reachable from outside the crate — `GamesManager` is not
    /// exported. Call
    /// [`GogDl::get_game_summary`](crate::GogDl::get_game_summary) instead,
    /// which delegates here internally.
    pub async fn get_game_summary(
        games_manager: &GamesManager,
        game_id: ProductId,
    ) -> Result<GameSummary, GamesError> {
        {
            let lock = games_manager.inner.lock().await;
            if let Some(game_summary) = lock.game_summary.get(&game_id) {
                return Ok(game_summary.clone());
            }
        }

        let url = games_manager.gamesdb_release_url(game_id);

        let game_summary: GameSummary =
            games_manager.client.fetch(&url, false, false, None).await?;

        let mut lock = games_manager.inner.lock().await;
        lock.game_summary.insert(game_id, game_summary.clone());

        Ok(game_summary)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // `get_game_summary` reads the same `gamesdb` record `get_owned_games`
    // does. A DLC's and a pack's decode fine; only their `type` ("dlc",
    // "spam") marks them as non-games, and `GameSummary` doesn't read it.
    // Step 10 reads it.
    #[test]
    fn a_non_games_summary_decodes_like_a_games() {
        for body in [
            include_str!("../../tests/fixtures/gamesdb_dlc.json"),
            include_str!("../../tests/fixtures/gamesdb_pack.json"),
            include_str!("../../tests/fixtures/gamesdb_game.json"),
        ] {
            let summary: GameSummary = serde_json::from_str(body).unwrap();
            assert!(!summary.summary.default.is_empty());
        }
    }
}
