use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::Mutex;

use crate::client::HttpClient;
use crate::games::OwnedGames;
use crate::games::error::GamesError;
use crate::games::game_build::GameBuilds;
use crate::games::game_details::GameDetails;
use crate::games::game_links::GameLinks;
use crate::games::game_screenshots::GameScreenshots;
use crate::games::game_summary::GameSummary;
use crate::games::owned_products::{OwnedProducts, ProductId};

#[derive(Clone)]
pub struct GamesManager {
    pub inner: Arc<Mutex<GamesManagerInner>>,
    pub client: HttpClient,
    /// Test-only: host the `gamesdb` lookups go to, so a fixture server can
    /// stand in for `gamesdb.gog.com`.
    #[cfg(test)]
    pub gamesdb_base: String,
    /// Test-only: host standing in for `embed.gog.com`.
    #[cfg(test)]
    pub embed_base: String,
    /// Test-only: host standing in for `content-system.gog.com`.
    #[cfg(test)]
    pub content_system_base: String,
    /// Test-only: host standing in for `api.gog.com`.
    #[cfg(test)]
    pub api_base: String,
}

pub struct GamesManagerInner {
    /// `None` until fetched, and again after `clear_cache`.
    pub owned_products: Option<OwnedProducts>,
    /// For details, links, builds and summary, `None` means the product is
    /// not a game; a lookup then answers `ProductNotAGame`.
    pub game_details: HashMap<ProductId, Option<GameDetails>>,
    pub game_links: HashMap<ProductId, Option<GameLinks>>,
    pub game_builds: HashMap<ProductId, Option<GameBuilds>>,
    pub game_summary: HashMap<ProductId, Option<GameSummary>>,
    pub game_screenshots: HashMap<ProductId, GameScreenshots>,
    /// `gamesdb`'s `type` per product. Only successful lookups are stored.
    pub product_types: HashMap<ProductId, String>,
}

impl GamesManager {
    pub fn new(client: HttpClient) -> Self {
        Self {
            inner: Arc::new(Mutex::new(GamesManagerInner {
                owned_products: None,
                game_details: HashMap::new(),
                game_links: HashMap::new(),
                game_builds: HashMap::new(),
                game_summary: HashMap::new(),
                game_screenshots: HashMap::new(),
                product_types: HashMap::new(),
            })),
            client,
            #[cfg(test)]
            gamesdb_base: "https://gamesdb.gog.com".to_string(),
            #[cfg(test)]
            embed_base: "https://embed.gog.com".to_string(),
            #[cfg(test)]
            content_system_base: "https://content-system.gog.com".to_string(),
            #[cfg(test)]
            api_base: "https://api.gog.com".to_string(),
        }
    }
    /// The `gamesdb` record URL for `id`.
    pub fn gamesdb_release_url(&self, id: ProductId) -> String {
        #[cfg(test)]
        let base = self.gamesdb_base.as_str();
        #[cfg(not(test))]
        let base = "https://gamesdb.gog.com";
        format!("{base}/platforms/gog/external_releases/{id}")
    }
    /// The `embed.gog.com` game details URL for `id`.
    pub fn game_details_url(&self, id: ProductId) -> String {
        #[cfg(test)]
        let base = self.embed_base.as_str();
        #[cfg(not(test))]
        let base = "https://embed.gog.com";
        format!("{base}/account/gameDetails/{id}.json")
    }
    /// The `api.gog.com` v2 record URL for `id`.
    pub fn game_links_url(&self, id: ProductId) -> String {
        #[cfg(test)]
        let base = self.api_base.as_str();
        #[cfg(not(test))]
        let base = "https://api.gog.com";
        format!("{base}/v2/games/{id}")
    }
    /// The owned-products list URL.
    pub fn owned_products_url(&self) -> String {
        #[cfg(test)]
        let base = self.embed_base.as_str();
        #[cfg(not(test))]
        let base = "https://embed.gog.com";
        format!("{base}/user/data/games")
    }
    /// The Windows build list URL for `id`.
    pub fn game_builds_url(&self, id: ProductId) -> String {
        #[cfg(test)]
        let base = self.content_system_base.as_str();
        #[cfg(not(test))]
        let base = "https://content-system.gog.com";
        format!("{base}/products/{id}/os/windows/builds?generation=2")
    }
    /// Drops the owned products and the game builds; everything else stays.
    /// Every cache is filled with a whole value after its fetch, outside the
    /// lock, so a fetch in flight can only put back its own complete result.
    pub async fn clear_cache(&self) {
        let mut lock = self.inner.lock().await;
        lock.owned_products = None;
        lock.game_builds.clear();
    }
    pub async fn get_owned_products(&self) -> Result<OwnedProducts, GamesError> {
        OwnedProducts::get_owned_products(self).await
    }
    pub async fn get_game_details(&self, game_id: ProductId) -> Result<GameDetails, GamesError> {
        GameDetails::get_game_details(self, game_id).await
    }
    pub async fn get_game_builds(&self, game_id: ProductId) -> Result<GameBuilds, GamesError> {
        GameBuilds::get_game_builds(self, game_id).await
    }
    pub async fn get_game_links(&self, game_id: ProductId) -> Result<GameLinks, GamesError> {
        GameLinks::get_game_links(self, game_id).await
    }
    pub async fn get_game_summary(&self, game_id: ProductId) -> Result<GameSummary, GamesError> {
        GameSummary::get_game_summary(self, game_id).await
    }
    pub async fn get_game_screenshots(
        &self,
        game_id: ProductId,
    ) -> Result<GameScreenshots, GamesError> {
        GameScreenshots::get_game_screenshots(self, game_id).await
    }
    pub async fn get_owned_games(&self) -> Result<OwnedGames, GamesError> {
        OwnedGames::get_owned_games(self).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{ChunkServer, Reply, authed_client};

    #[tokio::test]
    async fn clear_cache_drops_builds_and_keeps_the_rest() {
        let server = ChunkServer::start().await;
        let path = "/products/7/os/windows/builds";
        server.script(
            path,
            vec![Reply::Body(br#"{"count": 0, "items": []}"#.to_vec())],
        );
        let mut manager = GamesManager::new(authed_client().await);
        manager.content_system_base = server.base_url();
        manager.inner.lock().await.game_details.insert(7, None);
        manager
            .inner
            .lock()
            .await
            .product_types
            .insert(7, "game".to_string());

        manager.get_game_builds(7).await.unwrap();
        manager.get_game_builds(7).await.unwrap();
        assert_eq!(server.requests(path), 1);

        manager.clear_cache().await;
        manager.get_game_builds(7).await.unwrap();
        assert_eq!(server.requests(path), 2);

        let lock = manager.inner.lock().await;
        assert!(lock.game_details.contains_key(&7));
        assert!(lock.product_types.contains_key(&7));
    }

    const DLC: i32 = 1288586309;
    const PACK: i32 = 1563339319;
    const GAME: i32 = 1423049311;

    /// A fixture server serving step 9's captures on every endpoint.
    fn capture_server(server: &ChunkServer) {
        let rep = |b: &[u8]| (0..8).map(|_| Reply::Body(b.to_vec())).collect::<Vec<_>>();
        let nongame_builds = include_bytes!("../../tests/fixtures/game_builds_non_game.json");
        let details_empty = include_bytes!("../../tests/fixtures/game_details_non_game.json");
        for id in [DLC, PACK, GAME] {
            let (details, links, record, builds): (&[u8], &[u8], &[u8], &[u8]) = match id {
                DLC => (
                    details_empty,
                    include_bytes!("../../tests/fixtures/game_links_dlc.json"),
                    include_bytes!("../../tests/fixtures/gamesdb_dlc.json"),
                    nongame_builds,
                ),
                PACK => (
                    details_empty,
                    include_bytes!("../../tests/fixtures/game_links_pack.json"),
                    include_bytes!("../../tests/fixtures/gamesdb_pack.json"),
                    nongame_builds,
                ),
                _ => (
                    include_bytes!("../../tests/fixtures/game_details_game.json"),
                    include_bytes!("../../tests/fixtures/game_links_game.json"),
                    include_bytes!("../../tests/fixtures/gamesdb_game.json"),
                    include_bytes!("../../tests/fixtures/game_builds.json"),
                ),
            };
            server.script(&format!("/account/gameDetails/{id}.json"), rep(details));
            server.script(&format!("/v2/games/{id}"), rep(links));
            server.script(
                &format!("/platforms/gog/external_releases/{id}"),
                rep(record),
            );
            server.script(&format!("/products/{id}/os/windows/builds"), rep(builds));
        }
    }

    fn is_not_a_game<T>(result: Result<T, GamesError>) -> bool {
        matches!(result, Err(GamesError::ProductNotAGame))
    }

    async fn manager_on(server: &ChunkServer) -> GamesManager {
        let mut manager = GamesManager::new(authed_client().await);
        manager.gamesdb_base = server.base_url();
        manager.embed_base = server.base_url();
        manager.content_system_base = server.base_url();
        manager.api_base = server.base_url();
        manager
    }

    #[tokio::test]
    async fn every_fetcher_agrees_on_the_captured_products() {
        let server = ChunkServer::start().await;
        capture_server(&server);
        let manager = manager_on(&server).await;

        for _ in 0..2 {
            for id in [DLC, PACK] {
                assert!(is_not_a_game(manager.get_game_details(id).await));
                assert!(is_not_a_game(manager.get_game_links(id).await));
                assert!(is_not_a_game(manager.get_game_summary(id).await));
                assert!(is_not_a_game(manager.get_game_builds(id).await));
            }
            manager.get_game_details(GAME).await.unwrap();
            manager.get_game_links(GAME).await.unwrap();
            manager.get_game_summary(GAME).await.unwrap();
            assert!(
                !manager
                    .get_game_builds(GAME)
                    .await
                    .unwrap()
                    .items
                    .is_empty()
            );
        }

        // The second round came from the cache.
        for id in [DLC, PACK, GAME] {
            assert_eq!(
                server.requests(&format!("/account/gameDetails/{id}.json")),
                1
            );
            assert_eq!(server.requests(&format!("/v2/games/{id}")), 1);
            assert_eq!(
                server.requests(&format!("/products/{id}/os/windows/builds")),
                1
            );
        }
        // Summary and builds share one `gamesdb` record per non-game; a game
        // with builds never asks for its type from builds.
        for id in [DLC, PACK, GAME] {
            assert_eq!(
                server.requests(&format!("/platforms/gog/external_releases/{id}")),
                1
            );
        }
    }

    #[tokio::test]
    async fn game_details_carry_the_requested_id() {
        let server = ChunkServer::start().await;
        capture_server(&server);
        let manager = manager_on(&server).await;
        // Fresh, then cached.
        assert_eq!(manager.get_game_details(GAME).await.unwrap().id, GAME);
        assert_eq!(manager.get_game_details(GAME).await.unwrap().id, GAME);
        assert_eq!(
            server.requests(&format!("/account/gameDetails/{GAME}.json")),
            1
        );
    }

    #[tokio::test]
    async fn owned_games_agrees_with_the_fetchers() {
        let server = ChunkServer::start().await;
        capture_server(&server);
        server.script(
            "/user/data/games",
            vec![Reply::Body(
                format!(r#"{{"owned": [{DLC}, {PACK}, {GAME}]}}"#).into_bytes(),
            )],
        );
        let manager = manager_on(&server).await;
        assert_eq!(manager.get_owned_games().await.unwrap().owned, vec![GAME]);
    }

    #[tokio::test]
    async fn builds_reuse_the_type_summary_cached() {
        let server = ChunkServer::start().await;
        capture_server(&server);
        let manager = manager_on(&server).await;
        assert!(is_not_a_game(manager.get_game_summary(DLC).await));
        assert!(is_not_a_game(manager.get_game_builds(DLC).await));
        assert_eq!(
            server.requests(&format!("/platforms/gog/external_releases/{DLC}")),
            1
        );
    }

    #[tokio::test]
    async fn a_failed_type_lookup_for_empty_builds_is_not_cached() {
        let server = ChunkServer::start().await;
        let builds = include_bytes!("../../tests/fixtures/game_builds_non_game.json");
        let record = format!("/platforms/gog/external_releases/{DLC}");
        server.script(
            &format!("/products/{DLC}/os/windows/builds"),
            vec![Reply::Body(builds.to_vec()), Reply::Body(builds.to_vec())],
        );
        server.script(
            &record,
            vec![
                Reply::Status(404),
                Reply::Body(include_bytes!("../../tests/fixtures/gamesdb_dlc.json").to_vec()),
            ],
        );
        let manager = manager_on(&server).await;
        assert!(matches!(
            manager.get_game_builds(DLC).await,
            Err(GamesError::ClientError(_))
        ));
        assert!(is_not_a_game(manager.get_game_builds(DLC).await));
        assert_eq!(server.requests(&record), 2);
    }
}
