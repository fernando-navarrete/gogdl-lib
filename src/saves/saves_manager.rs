use std::{collections::HashMap, path::Path, sync::Arc};

use tokio::sync::{Mutex, mpsc};

use crate::{
    client::HttpClient,
    depot::DepotManager,
    games::GamesManager,
    saves::{
        GameSaveIds, SaveFile, SavesDownloadEvent, SavesUploadEvent,
        error::SavesError,
        remote_config::RemoteConfig,
        save_location::{ResolvedSaveLocation, resolve_locations},
        saves_auth::SavesAuth,
        saves_downloader::SavesDownloader,
        saves_uploader::SavesUploader,
    },
};

/// Crate-internal manager backing the cloud saves methods of `GogDl`
/// ([`get_save_files`](crate::GogDl::get_save_files),
/// [`get_remote_config`](crate::GogDl::get_remote_config),
/// [`download_save_files`](crate::GogDl::download_save_files) and
/// [`upload_save_files`](crate::GogDl::upload_save_files)). Not exported —
/// `GogDl` is the only entry point, mirroring `GamesManager` and
/// `DepotManager`.
pub struct SavesManager {
    pub inner: Arc<Mutex<SavesManagerInner>>,
    pub client: HttpClient,
}

/// The managers `SavesManager` resolves builds and build metadata through.
/// These are clones sharing state (and caches) with the ones held by
/// `GogDl` itself, plus the saves-specific caches.
pub struct SavesManagerInner {
    pub depot: DepotManager,
    pub games: GamesManager,
    /// Game-scoped auth grants keyed by `(game_id, build_name)`. Entries are
    /// reused while [`SavesAuth::is_valid`] holds and replaced on the next
    /// call once they expire.
    pub auth_cache: HashMap<(i32, String), SavesAuth>,
    /// Game client credentials keyed by `(game_id, build_name)`. Never
    /// evicted.
    pub ids_cache: HashMap<(i32, String), GameSaveIds>,
}

impl SavesManager {
    /// Builds a `SavesManager` around already-constructed shared managers.
    pub fn new(client: HttpClient, depot: DepotManager, games: GamesManager) -> Self {
        Self {
            inner: Arc::new(Mutex::new(SavesManagerInner {
                depot,
                games,
                auth_cache: HashMap::new(),
                ids_cache: HashMap::new(),
            })),
            client,
        }
    }
    /// Obtains a game-scoped cloud saves auth grant, cached per
    /// `(game_id, build_name)`. See [`SavesAuth::get_saves_auth`]. Not
    /// exposed publicly; used by [`get_save_files`](Self::get_save_files) and the
    /// download and upload paths.
    pub async fn get_saves_auth(
        &self,
        game_id: i32,
        build_name: &str,
    ) -> Result<SavesAuth, SavesError> {
        SavesAuth::get_saves_auth(self, game_id, build_name).await
    }
    /// Resolves the game's OAuth client credentials for `build_name`,
    /// cached per `(game_id, build_name)`. See
    /// [`GameSaveIds::get_game_save_ids`].
    pub async fn get_game_save_ids(
        &self,
        game_id: i32,
        build_name: &str,
    ) -> Result<GameSaveIds, SavesError> {
        GameSaveIds::get_game_save_ids(self, game_id, build_name).await
    }

    /// Fetches and parses the current user's cloud save listing for a game.
    /// See
    /// [`GogDl::get_save_files`](crate::GogDl::get_save_files) for the
    /// public-facing contract.
    pub async fn get_save_files(
        &self,
        game_id: i32,
        build_name: &str,
    ) -> Result<Vec<SaveFile>, SavesError> {
        SaveFile::get_save_files(self, game_id, build_name).await
    }

    /// Fetches the game's Galaxy client remote configuration, which
    /// declares whether it supports cloud saves and where they live. See
    /// [`GogDl::get_remote_config`](crate::GogDl::get_remote_config) for the
    /// public-facing contract.
    pub async fn get_remote_config(
        &self,
        game_id: i32,
        build_name: &str,
    ) -> Result<RemoteConfig, SavesError> {
        RemoteConfig::get_remote_config(self, game_id, build_name).await
    }

    /// Downloads every file in the game's cloud save listing into the
    /// directories its save locations expand to inside `prefix`. See
    /// [`GogDl::download_save_files`](crate::GogDl::download_save_files) for
    /// the public-facing contract.
    pub async fn download_save_files(
        &self,
        game_id: i32,
        build_name: &str,
        prefix: &Path,
        install_path: &Path,
        tx: mpsc::UnboundedSender<SavesDownloadEvent>,
    ) -> Result<(), SavesError> {
        let locations = self
            .resolve_save_locations(game_id, build_name, prefix, install_path)
            .await?;
        let auth = self.get_saves_auth(game_id, build_name).await?;
        let game_ids = self.get_game_save_ids(game_id, build_name).await?;
        let files = self.get_save_files(game_id, build_name).await?;

        SavesDownloader::new(self.client.clone(), auth, game_ids.client_id)
            .download_files(&files, &locations, tx)
            .await
    }

    /// Uploads every file under the directories the game's save locations
    /// expand to inside `prefix`. See
    /// [`GogDl::upload_save_files`](crate::GogDl::upload_save_files) for the
    /// public-facing contract.
    pub async fn upload_save_files(
        &self,
        game_id: i32,
        build_name: &str,
        prefix: &Path,
        install_path: &Path,
        tx: mpsc::UnboundedSender<SavesUploadEvent>,
    ) -> Result<(), SavesError> {
        let locations = self
            .resolve_save_locations(game_id, build_name, prefix, install_path)
            .await?;
        let auth = self.get_saves_auth(game_id, build_name).await?;
        let game_ids = self.get_game_save_ids(game_id, build_name).await?;

        SavesUploader::new(self.client.clone(), auth, game_ids.client_id)
            .upload_files(&locations, tx)
            .await
    }

    /// The game's declared save locations, expanded inside `prefix`. Done
    /// before anything is transferred so a location that cannot be resolved
    /// fails the call up front.
    async fn resolve_save_locations(
        &self,
        game_id: i32,
        build_name: &str,
        prefix: &Path,
        install_path: &Path,
    ) -> Result<Vec<ResolvedSaveLocation>, SavesError> {
        let remote_config = self.get_remote_config(game_id, build_name).await?;
        let locations = remote_config.get_locations()?;
        if locations.is_empty() {
            return Err(SavesError::CloudStorageNotSupported);
        }
        resolve_locations(&locations, prefix, install_path).await
    }
}
