use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::games::{GamesError, GamesManager, owned_products::ProductId};

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
/// Deserialize it from the builds listing. `game_title` is skipped by serde, so
/// it stays empty.
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
/// assert_eq!(builds.game_title, "");
/// ```
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct GameBuilds {
    /// Not populated. `#[serde(skip)]` and never assigned after
    /// deserialization — always the empty string.
    #[serde(skip)]
    pub game_title: String,
    /// The number of builds in `items`.
    pub count: i32,
    /// The builds themselves, in the order GOG returned them.
    pub items: Vec<GameBuild>,
}

impl GameBuilds {
    /// Not reachable from outside the crate — `GamesManager` is not
    /// exported. Call
    /// [`GogDl::get_game_builds`](crate::GogDl::get_game_builds) instead,
    /// which delegates here internally.
    pub async fn get_game_builds(
        games_manager: &GamesManager,
        game_id: ProductId,
    ) -> Result<Self, GamesError> {
        {
            let lock = games_manager.inner.lock().await;
            if let Some(game_builds) = lock.game_builds.get(&game_id) {
                return Ok(game_builds.clone());
            }
        }
        let url = format!(
            "https://content-system.gog.com/products/{}/os/windows/builds?generation=2",
            game_id
        );

        let game_builds: GameBuilds = games_manager.client.fetch(&url, false, true, None).await?;
        let mut lock = games_manager.inner.lock().await;
        lock.game_builds.insert(game_id, game_builds.clone());
        Ok(game_builds)
    }
}
