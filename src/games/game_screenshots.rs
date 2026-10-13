use serde::Deserialize;

use crate::games::{GamesError, GamesManager, owned_products::ProductId};

/// Raw screenshot links for a game, as returned by
/// [`GogDl::get_game_screenshots`](crate::GogDl::get_game_screenshots).
/// Resolve these to concrete URLs with
/// [`resolve_links`](Self::resolve_links) rather than reading `embedded`
/// directly — the links are templated and need formatter substitution.
#[derive(Deserialize, Clone, Debug)]
pub struct GameScreenshots {
    /// Raw embedded screenshot data, as returned by the API.
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

/// The formatter `resolve_links` prefers. GOG's screenshot links list
/// `product_card_screenshot_112`, `_748`, `_748_2x` and `1600`; the third
/// (the store-card image) is what the crate has always used.
const SCREENSHOT_FORMATTER: &str = "product_card_screenshot_748_2x";

impl GameScreenshots {
    /// Not reachable from outside the crate — `GamesManager` is not
    /// exported. Call
    /// [`GogDl::get_game_screenshots`](crate::GogDl::get_game_screenshots)
    /// instead, which delegates here internally.
    pub async fn get_game_screenshots(
        games_manager: &GamesManager,
        game_id: ProductId,
    ) -> Result<GameScreenshots, GamesError> {
        {
            let lock = games_manager.inner.lock().await;
            if let Some(game_screenshots) = lock.game_screenshots.get(&game_id) {
                return Ok(game_screenshots.clone());
            }
        }

        let url = format!("https://api.gog.com/v2/games/{}", game_id);

        let game_screenshots: GameScreenshots =
            games_manager.client.fetch(&url, false, false, None).await?;

        let mut lock = games_manager.inner.lock().await;
        lock.game_screenshots
            .insert(game_id, game_screenshots.clone());

        Ok(game_screenshots)
    }

    /// Resolves each screenshot link to a concrete URL. A link that isn't
    /// templated is returned as is. A templated one gets the
    /// `product_card_screenshot_748_2x` formatter (the store-card image, twice
    /// 748 px wide) if it lists it, otherwise its last formatter (GOG lists
    /// them smallest to largest). Only a templated link that lists no
    /// formatter at all is dropped, so the returned `Vec` can be shorter than
    /// `embedded.screenshots`.
    ///
    /// # Errors
    /// Never actually returns `Err` today; the `Result` is vestigial.
    pub fn resolve_links(&self) -> Result<Vec<String>, GamesError> {
        let mut links = Vec::new();
        for screenshot in &self.embedded.screenshots {
            let link = &screenshot.links.link;
            if !link.templated {
                links.push(link.link.clone());
                continue;
            }
            let formatters = link.formatters.as_deref().unwrap_or_default();
            let formatter = formatters
                .iter()
                .find(|f| *f == SCREENSHOT_FORMATTER)
                .or_else(|| formatters.last());
            if let Some(formatter) = formatter {
                links.push(link.link.replace("{formatter}", formatter));
            }
        }
        Ok(links)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screenshots(links: &[(&str, bool, Option<&[&str]>)]) -> GameScreenshots {
        GameScreenshots {
            embedded: Embedded {
                screenshots: links
                    .iter()
                    .map(|(href, templated, formatters)| Links {
                        links: Link {
                            link: ScreenshotLink {
                                link: href.to_string(),
                                templated: *templated,
                                formatters: formatters
                                    .map(|f| f.iter().map(|s| s.to_string()).collect()),
                            },
                        },
                    })
                    .collect(),
            },
        }
    }

    // Captured from `api.gog.com/v2/games/1423049311`: every screenshot of
    // every product sampled lists the same four formatters, and the old
    // `formatters.get(2)` picked the third.
    #[test]
    fn the_captured_screenshots_resolve_to_the_748_2x_image() {
        let parsed: GameScreenshots =
            serde_json::from_str(include_str!("../../tests/fixtures/game_screenshots.json"))
                .unwrap();
        let links = parsed.resolve_links().unwrap();
        assert_eq!(links.len(), 3);
        assert_eq!(
            links[0],
            "https://images.gog-statics.com/47bafd0276592912baca53ab789412d1b3545fa59d1f90bd94fa374779fb57e3_product_card_screenshot_748_2x.jpg"
        );
        assert!(
            links
                .iter()
                .all(|l| l.ends_with("_product_card_screenshot_748_2x.jpg"))
        );
    }

    #[test]
    fn the_formatter_is_chosen_by_name_not_position() {
        let s = screenshots(&[(
            "https://x/a_{formatter}.jpg",
            true,
            Some(&["1600", "product_card_screenshot_748_2x", "other"]),
        )]);
        assert_eq!(
            s.resolve_links().unwrap(),
            ["https://x/a_product_card_screenshot_748_2x.jpg"]
        );
    }

    #[test]
    fn a_link_without_the_named_formatter_falls_back_to_the_last() {
        let s = screenshots(&[(
            "https://x/a_{formatter}.jpg",
            true,
            Some(&["small", "large"]),
        )]);
        assert_eq!(s.resolve_links().unwrap(), ["https://x/a_large.jpg"]);
    }

    #[test]
    fn only_a_templated_link_with_no_formatter_is_dropped() {
        let s = screenshots(&[
            ("https://x/none_{formatter}.jpg", true, None),
            ("https://x/empty_{formatter}.jpg", true, Some(&[])),
            ("https://x/plain.jpg", false, None),
        ]);
        assert_eq!(s.resolve_links().unwrap(), ["https://x/plain.jpg"]);
    }
}
