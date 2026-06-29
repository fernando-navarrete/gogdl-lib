use std::sync::Arc;

use tokio::sync::Mutex;

use crate::auth::AuthManager;
use crate::client::HttpClient;
use crate::games::error::GamesError;
use crate::games::game_build::GameBuilds;
use crate::games::game_details::GameDetails;
use crate::games::owned_games::{GameId, OwnedGames};
use crate::games::product_details::ProductDetails;

pub struct GamesManager {
    pub inner: Arc<Mutex<GamesManagerInner>>,
    pub client: HttpClient,
}

pub struct GamesManagerInner {
    pub owned_games: OwnedGames,
    pub auth: AuthManager,
}

impl GamesManager {
    pub fn new(client: HttpClient, auth: AuthManager) -> Self {
        Self {
            inner: Arc::new(Mutex::new(GamesManagerInner {
                owned_games: OwnedGames::default(),
                auth,
            })),
            client,
        }
    }
    pub async fn get_owned_games(&self) -> Result<OwnedGames, GamesError> {
        OwnedGames::get_owned_games(self).await
    }
    pub async fn get_game_details(&self, game_id: GameId) -> Result<GameDetails, GamesError> {
        GameDetails::get_game_details(self, game_id).await
    }
    pub async fn get_game_builds(&self, game_id: GameId) -> Result<GameBuilds, GamesError> {
        GameBuilds::get_game_builds(self, game_id).await
    }
    pub async fn get_product_details(
        &self,
        product_id: &str,
    ) -> Result<ProductDetails, GamesError> {
        ProductDetails::get_product_details(self, product_id).await
    }
}
