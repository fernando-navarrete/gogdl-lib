use serde::Deserialize;

use crate::games::{GamesError, GamesManager, owned_games::GameId};

#[derive(Deserialize, Clone, Debug)]
pub struct GameScreenshots {
    #[serde(alias = "_embedded")]
    pub embedded: Embedded,
}

#[derive(Deserialize, Clone, Debug)]
pub struct Embedded {
    pub screenshots: Vec<Links>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct Links {
    #[serde(alias = "_links")]
    pub links: Link,
}

#[derive(Deserialize, Clone, Debug)]
pub struct Link {
    #[serde(alias = "self")]
    pub link: ScreenshotLink,
}

#[derive(Deserialize, Clone, Debug)]
pub struct ScreenshotLink {
    #[serde(alias = "href")]
    pub link: String,
    pub templated: bool,
    pub formatters: Option<Vec<String>>,
}

impl GameScreenshots {
    pub async fn get_game_screenshots(
        games_manager: &GamesManager,
        game_id: GameId,
    ) -> Result<GameScreenshots, GamesError> {
        {
            let lock = games_manager.inner.lock().await;
            if let Some(game_screenshots) = lock.game_screenshots.get(&game_id) {
                return Ok(game_screenshots.clone());
            }
        }

        let url = format!("https://api.gog.com/v2/games/{}", game_id);

        let game_screenshots: GameScreenshots =
            games_manager.client.fetch(&url, false, false).await?;

        let mut lock = games_manager.inner.lock().await;
        lock.game_screenshots
            .insert(game_id, game_screenshots.clone());

        Ok(game_screenshots)
    }

    pub fn resolve_links(&self) -> Result<Vec<String>, GamesError> {
        let mut links = Vec::new();
        for screenshot in &self.embedded.screenshots {
            if !screenshot.links.link.templated {
                links.push(screenshot.links.link.link.clone());
            } else {
                let formatter = {
                    if let Some(formatter) = screenshot.links.link.formatters.clone() {
                        if let Some(first) = formatter.get(2) {
                            first.clone()
                        } else {
                            continue;
                        }
                    } else {
                        continue;
                    }
                };
                let resolved_link = screenshot
                    .links
                    .link
                    .link
                    .replace("{formatter}", &formatter);
                links.push(resolved_link);
            }
        }
        Ok(links)
    }
}
