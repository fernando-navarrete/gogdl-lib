use std::{collections::HashMap, path::Path, sync::Arc};

use tokio::sync::{Mutex, mpsc};

use crate::{
    client::HttpClient,
    depot::DepotManager,
    games::GamesManager,
    saves::{
        GameSaveIds, SaveFile, SavesDownloadEvent, SavesUploadEvent, error::SavesError,
        remote_config::RemoteConfig, saves_auth::SavesAuth, saves_downloader::SavesDownloader,
        saves_uploader::SavesUploader,
    },
};

/// Crate-internal manager backing
/// [`GogDl::get_save_files`](crate::GogDl::get_save_files). Not exported —
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
    /// exposed publicly; used by [`get_save_files`](Self::get_save_files).
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

    /// Downloads every file in the game's cloud save listing into `path`.
    /// See
    /// [`GogDl::download_save_files`](crate::GogDl::download_save_files) for
    /// the public-facing contract.
    pub async fn download_save_files(
        &self,
        game_id: i32,
        build_name: &str,
        path: &Path,
        tx: mpsc::UnboundedSender<SavesDownloadEvent>,
    ) -> Result<(), SavesError> {
        let auth = self.get_saves_auth(game_id, build_name).await?;
        let game_ids = self.get_game_save_ids(game_id, build_name).await?;
        let files = self.get_save_files(game_id, build_name).await?;

        // Only used to tell a location segment from a real directory in the
        // cloud names. Without it every name still maps to a distinct path
        // (just keeping a location segment, if it has one), so a game whose
        // config is missing or unreadable is downloaded regardless.
        let locations = match self.get_remote_config(game_id, build_name).await {
            Ok(remote_config) => remote_config.get_locations().unwrap_or_default(),
            Err(_) => Vec::new(),
        };

        SavesDownloader::new(self.client.clone(), auth, game_ids.client_id)
            .download_files(&files, &locations, path, tx)
            .await
    }

    /// Uploads every file under `path` to the game's cloud saves. See
    /// [`GogDl::upload_save_files`](crate::GogDl::upload_save_files) for the
    /// public-facing contract.
    pub async fn upload_save_files(
        &self,
        game_id: i32,
        build_name: &str,
        path: &Path,
        tx: mpsc::UnboundedSender<SavesUploadEvent>,
    ) -> Result<(), SavesError> {
        let remote_config = self.get_remote_config(game_id, build_name).await?;
        let location = remote_config
            .get_locations()?
            .into_iter()
            .next()
            .ok_or(SavesError::CloudStorageNotSupported)?;
        let auth = self.get_saves_auth(game_id, build_name).await?;
        let game_ids = self.get_game_save_ids(game_id, build_name).await?;

        SavesUploader::new(self.client.clone(), auth, game_ids.client_id)
            .upload_files(path, &location.name, tx)
            .await
    }
}
