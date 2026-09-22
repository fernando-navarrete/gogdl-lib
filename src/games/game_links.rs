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
