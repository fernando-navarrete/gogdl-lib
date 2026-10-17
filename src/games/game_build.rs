use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::games::{GamesError, GamesManager, gamesdb::is_game_type, owned_products::ProductId};

/// A single published build of a game.
///
/// # Constructing in tests
///
/// Deserialize it from the JSON GOG returns; unknown keys are ignored.
///
/// ```
/// use gogdl_lib::GameBuild;
///
/// let build: GameBuild = serde_json::from_str(
///     r#"{"build_id": "1", "version_name": "2.31a",
///         "date_published": "2025-09-16T13:04:39+0000",
///         "link": "https://example.invalid/manifest"}"#,
/// )
/// .unwrap();
/// assert_eq!(build.version_name, "2.31a");
/// ```
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct GameBuild {
    /// GOG's internal build identifier.
    pub build_id: String,
    /// The human-readable version string — pass this as `build_name` to
    /// [`GogDl::get_downloadable_products`](crate::GogDl::get_downloadable_products)/
    /// [`GogDl::get_product_bundles`](crate::GogDl::get_product_bundles).
    pub version_name: String,
    /// When this build was published.
    pub date_published: DateTime<Utc>,
    /// The depot/build-metadata URL, used internally to resolve this
    /// build's depots.
    pub link: String,
}

/// The published build list for one game, as returned by
/// [`GogDl::get_game_builds`](crate::GogDl::get_game_builds). Always
/// reflects the Windows depot (`os/windows/builds`) — there is currently no
/// way to request a different OS.
///
/// # Constructing in tests
///
/// Deserialize it from the builds listing.
///
/// ```
/// use gogdl_lib::GameBuilds;
///
/// let builds: GameBuilds = serde_json::from_str(
///     r#"{"count": 1, "items": [{"build_id": "1", "version_name": "2.31a",
///         "date_published": "2025-09-16T13:04:39+0000",
///         "link": "https://example.invalid/manifest"}]}"#,
/// )
/// .unwrap();
/// assert_eq!(builds.items.len(), 1);
/// ```
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct GameBuilds {
    /// The number of builds in `items`.
    pub count: i32,
    /// The builds themselves, in the order GOG returned them.
    pub items: Vec<GameBuild>,
}

impl GameBuilds {
    /// Call
    /// [`GogDl::get_game_builds`](crate::GogDl::get_game_builds) instead,
    /// which delegates here internally.
    pub(crate) async fn get_game_builds(
        games_manager: &GamesManager,
        game_id: ProductId,
    ) -> Result<Self, GamesError> {
        {
            let lock = games_manager.inner.lock().await;
            if let Some(game_builds) = lock.game_builds.get(&game_id) {
                return game_builds.clone().ok_or(GamesError::ProductNotAGame);
            }
        }
        let url = games_manager.game_builds_url(game_id);

        let game_builds: GameBuilds = games_manager.client.fetch(&url, false, true, None).await?;

        // An empty list is what a DLC or a pack answers, and also what a game
        // with no published build answers, so `gamesdb`'s type decides. A
        // failed lookup caches nothing and is retried by the next call.
        let game_builds = if game_builds.items.is_empty()
            && !is_game_type(&games_manager.product_type(game_id).await?)
        {
            None
        } else {
            Some(game_builds)
        };

        let mut lock = games_manager.inner.lock().await;
        lock.game_builds.insert(game_id, game_builds.clone());
        game_builds.ok_or(GamesError::ProductNotAGame)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Captured from `content-system.gog.com/products/{id}/os/windows/builds`
    // for a DLC and a pack: 200 with `count: 0` and no items, the same
    // answer a game with no published build would give. `get_game_builds`
    // tells them apart by `gamesdb`'s type.
    #[test]
    fn a_non_games_builds_decode_as_an_empty_list() {
        let body = include_str!("../../tests/fixtures/game_builds_non_game.json");
        let builds: GameBuilds = serde_json::from_str(body).unwrap();
        assert_eq!(builds.count, 0);
        assert!(builds.items.is_empty());
    }
}
