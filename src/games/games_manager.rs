use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::Mutex;

use crate::auth::AuthManager;
use crate::client::HttpClient;
use crate::games::error::GamesError;
use crate::games::game_build::GameBuilds;
use crate::games::game_details::GameDetails;
use crate::games::game_links::GameLinks;
use crate::games::game_screenshots::GameScreenshots;
use crate::games::game_summary::GameSummary;
use crate::games::owned_games::{GameId, OwnedGames};

#[derive(Clone)]
pub struct GamesManager {
    pub inner: Arc<Mutex<GamesManagerInner>>,
    pub client: HttpClient,
}

pub struct GamesManagerInner {
    pub owned_games: OwnedGames,
    pub game_details: HashMap<GameId, Option<GameDetails>>,
    pub game_links: HashMap<GameId, GameLinks>,
    pub game_builds: HashMap<GameId, GameBuilds>,
    pub game_summary: HashMap<GameId, GameSummary>,
    pub game_screenshots: HashMap<GameId, GameScreenshots>,
    pub auth: AuthManager,
}

impl GamesManager {
    pub fn new(client: HttpClient, auth: AuthManager) -> Self {
        Self {
            inner: Arc::new(Mutex::new(GamesManagerInner {
                owned_games: OwnedGames::default(),
                game_details: HashMap::new(),
                game_links: HashMap::new(),
                game_builds: HashMap::new(),
                game_summary: HashMap::new(),
                game_screenshots: HashMap::new(),
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
    pub async fn get_game_links(&self, game_id: GameId) -> Result<GameLinks, GamesError> {
        GameLinks::get_game_links(self, game_id).await
    }
    pub async fn get_game_summary(&self, game_id: GameId) -> Result<GameSummary, GamesError> {
        GameSummary::get_game_summary(self, game_id).await
    }
    pub async fn get_game_screenshots(
        &self,
        game_id: GameId,
    ) -> Result<GameScreenshots, GamesError> {
        GameScreenshots::get_game_screenshots(self, game_id).await
    }
}
