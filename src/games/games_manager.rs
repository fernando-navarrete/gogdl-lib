use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::Mutex;

use crate::client::HttpClient;
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
}

pub struct GamesManagerInner {
    pub owned_products: OwnedProducts,
    pub game_details: HashMap<ProductId, Option<GameDetails>>,
    pub game_links: HashMap<ProductId, GameLinks>,
    pub game_builds: HashMap<ProductId, GameBuilds>,
    pub game_summary: HashMap<ProductId, GameSummary>,
    pub game_screenshots: HashMap<ProductId, GameScreenshots>,
}

impl GamesManager {
    pub fn new(client: HttpClient) -> Self {
        Self {
            inner: Arc::new(Mutex::new(GamesManagerInner {
                owned_products: OwnedProducts::default(),
                game_details: HashMap::new(),
                game_links: HashMap::new(),
                game_builds: HashMap::new(),
                game_summary: HashMap::new(),
                game_screenshots: HashMap::new(),
            })),
            client,
        }
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
}
