use std::sync::Arc;

use tokio::sync::Mutex;

use crate::{
    client::HttpClient,
    depot::DepotManager,
    games::GamesManager,
    saves::{error::SavesError, saves_auth::SavesAuth},
};

pub struct SavesManager {
    pub inner: Arc<Mutex<SavesManagerInner>>,
    pub client: HttpClient,
}

pub struct SavesManagerInner {
    pub depot: DepotManager,
    pub games: GamesManager,
}

impl SavesManager {
    pub fn new(client: HttpClient, depot: DepotManager, games: GamesManager) -> Self {
        Self {
            inner: Arc::new(Mutex::new(SavesManagerInner { depot, games })),
            client,
        }
    }
    pub async fn get_saves_auth(
        &self,
        game_id: i32,
        build_name: &str,
    ) -> Result<SavesAuth, SavesError> {
        SavesAuth::get_saves_auth(self, game_id, build_name).await
    }
}
