use crate::DepotInfo;
use crate::auth::AuthManager;
use crate::client::HttpClient;
use crate::depot::{BuildMetadata, DepotManager, ProductDetails};
use crate::games::{GameBuilds, GameDetails, GamesManager, OwnedGames};
use crate::gogdl::error::GogDlError;

pub struct GogDl {
    auth: AuthManager,
    games: GamesManager,
    depot: DepotManager,
}

impl GogDl {
    pub fn new_from_client(client: reqwest::Client) -> Self {
        let http_client = HttpClient::new_with_client(client);
        let auth_manager = AuthManager::new(http_client.clone());
        let games_manager = GamesManager::new(http_client.clone(), auth_manager.clone());
        let depot_manager = DepotManager::new(http_client.clone(), auth_manager.clone());
        Self {
            auth: auth_manager,
            games: games_manager,
            depot: depot_manager,
        }
    }
    pub fn get_login_url(&self) -> &str {
        self.auth.get_login_url()
    }
    pub async fn refresh_auth(&self) -> Result<(), GogDlError> {
        self.auth.refresh_auth().await?;
        Ok(())
    }
    pub async fn login_with_code(&self, code: &str) -> Result<(), GogDlError> {
        self.auth.login_with_code(code).await?;
        Ok(())
    }
    pub async fn get_owned_games(&self) -> Result<OwnedGames, GogDlError> {
        let owned_games = self.games.get_owned_games().await?;
        Ok(owned_games)
    }
    pub async fn get_game_details(&self, game_id: i32) -> Result<GameDetails, GogDlError> {
        let game_details = self.games.get_game_details(game_id).await?;
        Ok(game_details)
    }
    pub async fn get_game_builds(&self, game_id: i32) -> Result<GameBuilds, GogDlError> {
        let game_builds = self.games.get_game_builds(game_id).await?;
        Ok(game_builds)
    }
    pub async fn get_build_metadata(&self, game_link: &str) -> Result<BuildMetadata, GogDlError> {
        let build_metadata = self.depot.get_build_metadata(game_link).await?;
        Ok(build_metadata)
    }
    pub async fn get_product_details(
        &self,
        product_id: &str,
    ) -> Result<ProductDetails, GogDlError> {
        let product_details = self.depot.get_product_details(product_id).await?;
        Ok(product_details)
    }
    pub async fn get_depot_info(&self, depot_manifest: &str) -> Result<DepotInfo, GogDlError> {
        let depot_info = self.depot.get_depot_info(depot_manifest).await?;
        Ok(depot_info)
    }
}
