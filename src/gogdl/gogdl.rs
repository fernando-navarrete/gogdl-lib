use crate::auth::AuthManager;
use crate::client::HttpClient;
use crate::depot::{DepotManager, ProductDetails};
use crate::downloader::{DownloadManager, DownloadableProduct, ProductBundle};
use crate::games::{
    GameBuilds, GameDetails, GameLinks, GameScreenshots, GameSummary, GamesManager, OwnedGames,
};
use crate::gogdl::error::GogDlError;
use crate::secure_links::{SecureLinks, SecureLinksManager};

pub struct GogDl {
    auth: AuthManager,
    games: GamesManager,
    depot: DepotManager,
    secure_links: SecureLinksManager,
    downloader: DownloadManager,
}

impl GogDl {
    pub fn new_from_client(client: reqwest::Client) -> Self {
        let http_client = HttpClient::new_with_client(client);
        let auth_manager = AuthManager::new(http_client.clone());
        let games_manager = GamesManager::new(http_client.clone(), auth_manager.clone());
        let depot_manager = DepotManager::new(http_client.clone(), auth_manager.clone());
        let secure_links_manager = SecureLinksManager::new(
            http_client.clone(),
            auth_manager.clone(),
            games_manager.clone(),
        );
        let download_manager = DownloadManager::new(
            auth_manager.clone(),
            depot_manager.clone(),
            secure_links_manager.clone(),
            games_manager.clone(),
            http_client.clone(),
        );

        Self {
            auth: auth_manager,
            games: games_manager,
            depot: depot_manager,
            secure_links: secure_links_manager,
            downloader: download_manager,
        }
    }
    pub async fn restore_auth(&self, json_str: &str) -> Result<(), GogDlError> {
        self.auth.restore_from_string(json_str).await?;
        Ok(())
    }
    pub fn get_login_url(&self) -> &str {
        self.auth.get_login_url()
    }
    pub async fn refresh_auth(&self) -> Result<String, GogDlError> {
        let auth_string = self.auth.refresh_auth().await?;
        Ok(auth_string)
    }
    pub async fn login_with_code(&self, code: &str) -> Result<String, GogDlError> {
        let auth_string = self.auth.login_with_code(code).await?;
        Ok(auth_string)
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
    pub async fn get_game_links(&self, game_id: i32) -> Result<GameLinks, GogDlError> {
        let game_links = self.games.get_game_links(game_id).await?;
        Ok(game_links)
    }
    pub async fn get_game_summary(&self, game_id: i32) -> Result<GameSummary, GogDlError> {
        let game_summary = self.games.get_game_summary(game_id).await?;
        Ok(game_summary)
    }
    pub async fn get_game_screenshots(&self, game_id: i32) -> Result<GameScreenshots, GogDlError> {
        let game_screenshots = self.games.get_game_screenshots(game_id).await?;
        Ok(game_screenshots)
    }
    pub async fn get_secure_links(&self, game_id: &str) -> Result<SecureLinks, GogDlError> {
        let secure_links = self.secure_links.get_secure_links(game_id).await?;
        Ok(secure_links)
    }
    pub async fn get_product_details(
        &self,
        product_id: &str,
    ) -> Result<ProductDetails, GogDlError> {
        let product_details = self.depot.get_product_details(product_id).await?;
        Ok(product_details)
    }
    pub async fn get_downloadable_products(
        &self,
        game_id: i32,
        build_name: &str,
    ) -> Result<Vec<DownloadableProduct>, GogDlError> {
        let downloadable_products = self
            .downloader
            .get_downloadable_products(game_id, build_name)
            .await?;
        Ok(downloadable_products)
    }
    pub async fn get_product_bundles(
        &self,
        game_id: i32,
        build_name: &str,
        selected_products: &[i32],
    ) -> Result<Vec<ProductBundle>, GogDlError> {
        let downloadable_files = self
            .downloader
            .get_downloadable_files(game_id, build_name, selected_products)
            .await?;
        Ok(downloadable_files)
    }
}
