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

/// What `api.gog.com/v2/games/{id}` is read as: the links, plus
/// `_embedded.productType`, the only thing that tells a DLC or a pack (same
/// shape, still a 200) from a game.
#[derive(Deserialize)]
struct LinksRecord {
    #[serde(rename = "_links")]
    links: Links,
    #[serde(rename = "_embedded")]
    embedded: Embedded,
}

#[derive(Deserialize)]
struct Embedded {
    #[serde(rename = "productType")]
    product_type: String,
}

/// `_embedded.productType` of a game on `api.gog.com/v2/games/{id}`; captures
/// of a DLC and a pack say `DLC` and `PACK`. Undocumented, like `gamesdb`.
const LINKS_GAME_TYPE: &str = "GAME";

fn is_game_product_type(product_type: &str) -> bool {
    product_type == LINKS_GAME_TYPE
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
            if let Some(game_links) = lock.game_links.get(&game_id) {
                return game_links.clone().ok_or(GamesError::ProductNotAGame);
            }
        }

        let url = games_manager.game_links_url(game_id);

        let record: LinksRecord = games_manager.client.fetch(&url, false, false, None).await?;
        let game_links = is_game_product_type(&record.embedded.product_type).then_some(GameLinks {
            links: record.links,
        });

        let mut lock = games_manager.inner.lock().await;
        lock.game_links.insert(game_id, game_links.clone());

        game_links.ok_or(GamesError::ProductNotAGame)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Captured from `api.gog.com/v2/games/{id}`: a DLC and a pack answer 200
    // with the same shape as a game. Only `_embedded.productType` ("DLC",
    // "PACK", "GAME") tells them apart, and `get_game_links` reads it.
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
        assert_eq!(
            types
                .iter()
                .map(|t| is_game_product_type(t))
                .collect::<Vec<_>>(),
            [false, false, true]
        );
    }
}
