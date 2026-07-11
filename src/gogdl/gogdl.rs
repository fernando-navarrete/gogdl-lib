use std::path::Path;

use tokio::sync::mpsc;

use crate::DownloadJobEvent;
use crate::RepairEvent;
use crate::VerifyEvent;
use crate::auth::AuthManager;
use crate::client::HttpClient;
use crate::depot::{DepotManager, ProductDetails};
use crate::downloader::{DownloadManager, DownloadableFiles, DownloadableProduct};
use crate::games::{
    GameBuilds, GameDetails, GameLinks, GameScreenshots, GameSummary, GamesManager, OwnedGames,
};
use crate::gogdl::error::GogDlError;
use crate::saves::{RemoteConfig, SaveFile, SaveProgress, SavesManager};
use crate::secure_links::{SecureLinks, SecureLinksManager};

pub struct GogDl {
    auth: AuthManager,
    games: GamesManager,
    depot: DepotManager,
    secure_links: SecureLinksManager,
    downloader: DownloadManager,
    saves: SavesManager,
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
        let saves_manager = SavesManager::new(
            http_client.clone(),
            auth_manager.clone(),
            games_manager.clone(),
            depot_manager.clone(),
        );

        Self {
            auth: auth_manager,
            games: games_manager,
            depot: depot_manager,
            secure_links: secure_links_manager,
            downloader: download_manager,
            saves: saves_manager,
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
    pub async fn verify_files(
        &self,
        files: Vec<DownloadableFiles>,
        path: &str,
        tx: mpsc::UnboundedSender<VerifyEvent>,
    ) -> Result<(), GogDlError> {
        self.downloader.verify_download(path, files, tx).await?;
        Ok(())
    }
    pub async fn repair_files(
        &self,
        files: Vec<DownloadableFiles>,
        path: &str,
        tx: mpsc::UnboundedSender<RepairEvent>,
    ) -> Result<(), GogDlError> {
        self.downloader.repair_download(path, files, tx).await?;
        Ok(())
    }
    pub async fn download_files(
        &self,
        files: Vec<DownloadableFiles>,
        path: &str,
        tx: mpsc::UnboundedSender<DownloadJobEvent>,
    ) -> Result<(), GogDlError> {
        self.downloader.download(path, files, tx).await?;
        Ok(())
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
    pub async fn get_downloadable_files(
        &self,
        game_id: i32,
        build_name: &str,
        selected_products: &[&str],
    ) -> Result<Vec<DownloadableFiles>, GogDlError> {
        let downloadable_files = self
            .downloader
            .get_downloadable_files(game_id, build_name, selected_products)
            .await?;
        Ok(downloadable_files)
    }
    pub async fn get_remote_config(&self, client_id: &str) -> Result<RemoteConfig, GogDlError> {
        let remote_config = self.saves.get_remote_config(client_id).await?;
        Ok(remote_config)
    }
    pub async fn get_save_auth_ids(&self, game_id: i32) -> Result<(String, String), GogDlError> {
        let auth_ids = self.saves.get_auth_ids(game_id).await?;
        Ok(auth_ids)
    }
    pub async fn get_save_file_list(
        &self,
        client_id: &str,
        client_secret: &str,
    ) -> Result<Vec<SaveFile>, GogDlError> {
        let save_files = self
            .saves
            .get_save_file_list(client_id, client_secret)
            .await?;
        Ok(save_files)
    }
    pub async fn download_save_file(
        &self,
        save_file: &SaveFile,
        client_id: &str,
        client_secret: &str,
        path: &Path,
        tx: mpsc::UnboundedSender<SaveProgress>,
    ) -> Result<(), GogDlError> {
        self.saves
            .download_save_file(save_file, client_id, client_secret, path, tx)
            .await?;
        Ok(())
    }
    pub async fn upload_save_file(
        &self,
        client_id: &str,
        client_secret: &str,
        path: &Path,
        url_path: &str,
        tx: mpsc::UnboundedSender<SaveProgress>,
    ) -> Result<(), GogDlError> {
        self.saves
            .upload_save_file(client_id, client_secret, path, url_path, tx)
            .await?;
        Ok(())
    }
    pub async fn delete_save_file(
        &self,
        save_file: &SaveFile,
        client_id: &str,
        client_secret: &str,
    ) -> Result<(), GogDlError> {
        self.saves
            .delete_save_file(save_file, client_id, client_secret)
            .await?;
        Ok(())
    }
}
