use serde::Deserialize;

use crate::games::{GamesError, GamesManager, owned_products::ProductId};

/// Image links for a game, as returned by
/// [`GogDl::get_game_links`](crate::GogDl::get_game_links).
#[derive(Deserialize, Clone)]
pub struct GameLinks {
    /// The box art, background and Galaxy background image links.
    #[serde(alias = "_links")]
    pub links: Links,
}

#[derive(Deserialize, Clone)]
pub struct Links {
    #[serde(alias = "boxArtImage")]
    pub box_art_image: GogImage,
    #[serde(alias = "backgroundImage")]
    pub background_image: GogImage,
    #[serde(alias = "galaxyBackgroundImage")]
    pub galaxy_background_image: GogImage,
}

#[derive(Deserialize, Clone)]
pub struct GogImage {
    pub href: String,
}

impl GameLinks {
    /// Not reachable from outside the crate — `GamesManager` is not
    /// exported. Call
    /// [`GogDl::get_game_links`](crate::GogDl::get_game_links) instead,
    /// which delegates here internally.
    pub async fn get_game_links(
        games_manager: &GamesManager,
        game_id: ProductId,
    ) -> Result<GameLinks, GamesError> {
        {
            let lock = games_manager.inner.lock().await;
            if let Some(game_details) = lock.game_links.get(&game_id) {
                return Ok(game_details.clone());
            }
        }

        let url = format!("https://api.gog.com/v2/games/{}", game_id);

        let game_links: GameLinks = games_manager.client.fetch(&url, false, false, None).await?;

        let mut lock = games_manager.inner.lock().await;
        lock.game_links.insert(game_id, game_links.clone());

        Ok(game_links)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Captured from `api.gog.com/v2/games/{id}`: a DLC and a pack answer 200
    // with the same shape as a game. Only `_embedded.productType` ("DLC",
    // "PACK", "GAME") tells them apart, and `GameLinks` doesn't read it.
    // Step 10 reads it.
    #[test]
    fn a_non_games_links_decode_like_a_games() {
        for body in [
            include_str!("../../tests/fixtures/game_links_dlc.json"),
            include_str!("../../tests/fixtures/game_links_pack.json"),
            include_str!("../../tests/fixtures/game_links_game.json"),
        ] {
            let links: GameLinks = serde_json::from_str(body).unwrap();
            assert!(links.links.box_art_image.href.starts_with("https://"));
        }
    }

    #[test]
    fn the_product_type_marks_a_non_game() {
        let types: Vec<String> = [
            include_str!("../../tests/fixtures/game_links_dlc.json"),
            include_str!("../../tests/fixtures/game_links_pack.json"),
            include_str!("../../tests/fixtures/game_links_game.json"),
        ]
        .iter()
        .map(|body| {
            let value: serde_json::Value = serde_json::from_str(body).unwrap();
            value["_embedded"]["productType"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
        assert_eq!(types, ["DLC", "PACK", "GAME"]);
    }
}
