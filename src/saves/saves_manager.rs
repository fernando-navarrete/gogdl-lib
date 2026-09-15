use std::{collections::HashMap, sync::Arc};

use tokio::sync::Mutex;

use crate::{
    client::HttpClient,
    depot::DepotManager,
    games::GamesManager,
    saves::{GameSaveIds, SaveFiles, error::SavesError, saves_auth::SavesAuth},
};

/// Crate-internal manager backing
/// [`GogDl::get_saves_auth`](crate::GogDl::get_saves_auth). Not exported —
/// `GogDl` is the only entry point, mirroring `GamesManager` and
/// `DepotManager`.
pub struct SavesManager {
    pub inner: Arc<Mutex<SavesManagerInner>>,
    pub client: HttpClient,
}

/// The managers `SavesManager` resolves builds and build metadata through.
/// These are clones sharing state (and caches) with the ones held by
/// `GogDl` itself.
pub struct SavesManagerInner {
    pub depot: DepotManager,
    pub games: GamesManager,
    pub auth_cache: HashMap<(i32, String), SavesAuth>,
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
    /// Obtains a game-scoped cloud saves auth grant. See
    /// [`GogDl::get_saves_auth`](crate::GogDl::get_saves_auth) for the
    /// public-facing contract.
    pub async fn get_saves_auth(
        &self,
        game_id: i32,
        build_name: &str,
    ) -> Result<SavesAuth, SavesError> {
        SavesAuth::get_saves_auth(self, game_id, build_name).await
    }
    pub async fn get_game_save_ids(
        &self,
        game_id: i32,
        build_name: &str,
    ) -> Result<GameSaveIds, SavesError> {
        GameSaveIds::get_game_save_ids(self, game_id, build_name).await
    }

    pub async fn get_save_files(
        &self,
        game_id: i32,
        build_name: &str,
    ) -> Result<SaveFiles, SavesError> {
        SaveFiles::get_save_files(self, game_id, build_name).await
    }
}
