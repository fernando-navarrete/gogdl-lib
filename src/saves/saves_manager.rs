use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use tokio::sync::{Mutex, mpsc::UnboundedSender};

use crate::{
    auth::{AuthManager, SavesAuth},
    client::HttpClient,
    depot::DepotManager,
    games::{GameBuilds, GamesManager},
    saves::{
        error::SavesError,
        remote_config::RemoteConfig,
        save_file::{SaveFile, SaveProgress},
    },
};

#[derive(Clone)]
pub struct SavesManager {
    pub inner: Arc<Mutex<SavesManagerInner>>,
    pub client: HttpClient,
}

pub struct SavesManagerInner {
    pub auth: AuthManager,
    pub games: GamesManager,
    pub depot: DepotManager,
    /// Keyed by Galaxy client id. Safe to cache indefinitely: a client id's
    /// remote-config document doesn't change between calls.
    pub remote_config: HashMap<String, RemoteConfig>,
    /// Keyed by game id. Safe to cache indefinitely: a game's cloud-save
    /// `(client_id, client_secret)` pair is tied to its Galaxy client
    /// registration, not anything that changes at runtime.
    pub auth_ids: HashMap<i32, (String, String)>,
}

/// Cloud-save client_id/client_secret are tied to the product's Galaxy
/// client registration, not the OS build — the first build is used as a
/// stable default since GOG's save/remote-config model is itself
/// Windows-centric and every build shares the same cloud-save credentials.
fn first_build_link(builds: &GameBuilds) -> Result<&str, SavesError> {
    builds
        .items
        .first()
        .map(|build| build.link.as_str())
        .ok_or(SavesError::BuildNotFound)
}

impl SavesManager {
    pub fn new(
        client: HttpClient,
        auth: AuthManager,
        games: GamesManager,
        depot: DepotManager,
    ) -> Self {
        Self {
            inner: Arc::new(Mutex::new(SavesManagerInner {
                auth,
                games,
                depot,
                remote_config: HashMap::new(),
                auth_ids: HashMap::new(),
            })),
            client,
        }
    }
    /// Fetches the Galaxy client remote-config document for `client_id`:
    /// whether cloud saves are enabled and where they live locally.
    pub async fn get_remote_config(&self, client_id: &str) -> Result<RemoteConfig, SavesError> {
        RemoteConfig::get_remote_config(self, client_id).await
    }
    /// Returns the `(client_id, client_secret)` a game's cloud saves
    /// authenticate with, derived from its first build's metadata.
    pub async fn get_auth_ids(&self, game_id: i32) -> Result<(String, String), SavesError> {
        {
            let lock = self.inner.lock().await;
            if let Some(ids) = lock.auth_ids.get(&game_id) {
                return Ok(ids.clone());
            }
        }
        let (games, depot) = {
            let lock = self.inner.lock().await;
            (lock.games.clone(), lock.depot.clone())
        };
        let game_builds = games.get_game_builds(game_id).await?;
        let build_link = first_build_link(&game_builds)?.to_string();
        let build_metadata = depot.get_build_metadata(&build_link).await?;
        let ids = (build_metadata.client_id, build_metadata.client_secret);

        let mut lock = self.inner.lock().await;
        lock.auth_ids.insert(game_id, ids.clone());
        Ok(ids)
    }
    /// Exchanges the user's refresh token for a save-scoped `SavesAuth`
    /// against this game's own cloud-save credentials. Minted fresh on every
    /// call rather than cached, since it's cheap and short-lived by design.
    async fn mint_saves_auth(
        &self,
        client_id: &str,
        client_secret: &str,
    ) -> Result<SavesAuth, SavesError> {
        let auth = {
            let lock = self.inner.lock().await;
            lock.auth.clone()
        };
        let saves_auth = auth.get_cloud_saves_tokens(client_id, client_secret).await?;
        Ok(saves_auth)
    }
    pub async fn get_save_file_list(
        &self,
        client_id: &str,
        client_secret: &str,
    ) -> Result<Vec<SaveFile>, SavesError> {
        let saves_auth = self.mint_saves_auth(client_id, client_secret).await?;
        SaveFile::list(self, &saves_auth, client_id).await
    }
    pub async fn download_save_file(
        &self,
        save_file: &SaveFile,
        client_id: &str,
        client_secret: &str,
        path: &Path,
        tx: UnboundedSender<SaveProgress>,
    ) -> Result<(), SavesError> {
        let saves_auth = self.mint_saves_auth(client_id, client_secret).await?;
        save_file.download(self, &saves_auth, path, tx).await
    }
    pub async fn upload_save_file(
        &self,
        client_id: &str,
        client_secret: &str,
        path: &Path,
        url_path: &str,
        tx: UnboundedSender<SaveProgress>,
    ) -> Result<(), SavesError> {
        let saves_auth = self.mint_saves_auth(client_id, client_secret).await?;
        SaveFile::upload(self, &saves_auth, path, url_path, tx).await
    }
    pub async fn delete_save_file(
        &self,
        save_file: &SaveFile,
        client_id: &str,
        client_secret: &str,
    ) -> Result<(), SavesError> {
        let saves_auth = self.mint_saves_auth(client_id, client_secret).await?;
        save_file.delete(self, &saves_auth).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::games::GameBuild;
    use chrono::Utc;

    fn builds_with_links(links: &[&str]) -> GameBuilds {
        GameBuilds {
            game_title: String::new(),
            count: links.len() as i32,
            items: links
                .iter()
                .enumerate()
                .map(|(i, link)| GameBuild {
                    build_id: i.to_string(),
                    version_name: format!("v{i}"),
                    date_published: Utc::now(),
                    link: link.to_string(),
                })
                .collect(),
        }
    }

    #[test]
    fn first_build_link_returns_first_item() {
        let builds = builds_with_links(&["https://example.com/build"]);
        assert_eq!(
            first_build_link(&builds).unwrap(),
            "https://example.com/build"
        );
    }

    #[test]
    fn first_build_link_errors_on_empty_items() {
        let builds = builds_with_links(&[]);
        assert!(matches!(first_build_link(&builds), Err(SavesError::BuildNotFound)));
    }
}
