use std::{collections::HashMap, sync::Arc};

use tokio::sync::Mutex;

use crate::{
    auth::AuthManager,
    client::HttpClient,
    games::GamesManager,
    secure_links::{error::SecureLinksError, secure_links::SecureLinks},
};

#[derive(Clone)]
pub struct SecureLinksManager {
    pub inner: Arc<Mutex<SecureLinksManagerInner>>,
    pub client: HttpClient,
}

pub struct SecureLinksManagerInner {
    pub auth: AuthManager,
    pub games: GamesManager,
    links_cache: HashMap<String, SecureLinks>,
}

impl SecureLinksManager {
    pub fn new(client: HttpClient, auth: AuthManager, games: GamesManager) -> Self {
        Self {
            inner: Arc::new(Mutex::new(SecureLinksManagerInner {
                auth,
                games,
                links_cache: HashMap::new(),
            })),
            client,
        }
    }
    pub async fn get_secure_links(&self, game_id: &str) -> Result<SecureLinks, SecureLinksError> {
        let available_games = {
            let lock = self.inner.lock().await;
            let games = lock.games.get_owned_games().await?;
            games.owned
        };

        {
            let lock = self.inner.lock().await;
            if let Some(links) = lock.links_cache.get(game_id) {
                return Ok(links.clone());
            }
        }

        if !available_games.contains(&game_id.parse().unwrap()) {
            return Err(SecureLinksError::ProductNotOwned(game_id.to_string()));
        }
        let secure_links = SecureLinks::get_secure_links(self, game_id).await?;

        {
            let mut lock = self.inner.lock().await;
            lock.links_cache
                .insert(game_id.to_string(), secure_links.clone());
        }

        Ok(secure_links)
    }
}
