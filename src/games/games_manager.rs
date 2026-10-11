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
}

pub struct GamesManagerInner {
    /// `None` until fetched, and again after `clear_cache`.
    pub owned_products: Option<OwnedProducts>,
    pub game_details: HashMap<ProductId, Option<GameDetails>>,
    pub game_links: HashMap<ProductId, GameLinks>,
    pub game_builds: HashMap<ProductId, GameBuilds>,
    pub game_summary: HashMap<ProductId, GameSummary>,
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
}
