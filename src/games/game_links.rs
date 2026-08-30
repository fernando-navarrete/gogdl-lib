use serde::Deserialize;

use crate::{
    client::Request,
    games::{GamesError, GamesManager, owned_games::GameId},
};

#[derive(Deserialize, Clone)]
pub struct GameLinks {
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
    pub async fn get_game_links(
        games_manager: &GamesManager,
        game_id: GameId,
    ) -> Result<GameLinks, GamesError> {
        {
            let lock = games_manager.inner.lock().await;
            if let Some(game_details) = lock.game_links.get(&game_id) {
                return Ok(game_details.clone());
            }
        }

        let url = format!("https://api.gog.com/v2/games/{}", game_id);

        let game_links: GameLinks = match games_manager.client.fetch(Request::Get { url }).await {
            Ok(game_links) => game_links,
            Err(err) => {
                return Err(GamesError::from(err));
            }
        };

        let mut lock = games_manager.inner.lock().await;
        lock.game_links.insert(game_id, game_links.clone());

        Ok(game_links)
    }
}
