use std::{collections::HashMap, sync::Arc};

use tokio::sync::Mutex;

use crate::{
    client::HttpClient,
    games::GamesManager,
    secure_links::{error::SecureLinksError, secure_links::SecureLinks},
};

/// Fetches and caches [`SecureLinks`] per `game_id`, checking ownership on a
/// fresh fetch and re-fetching automatically once a cached entry expires.
#[derive(Clone)]
pub struct SecureLinksManager {
    /// Shared owned-games handle and link cache. `pub` for crate-internal
    /// reuse; not meant to be reached from outside the crate.
    pub inner: Arc<Mutex<SecureLinksManagerInner>>,
    /// HTTP client used to fetch fresh secure links.
    pub client: HttpClient,
}

/// State behind [`SecureLinksManager`]'s mutex: the owned-games lookup and
/// the per-`game_id` link cache.
pub struct SecureLinksManagerInner {
    /// Used to confirm the account owns `game_id` before fetching its links.
    pub games: GamesManager,
    links_cache: HashMap<String, SecureLinks>,
}

impl SecureLinksManager {
    /// Builds a manager with an empty link cache.
    pub fn new(client: HttpClient, games: GamesManager) -> Self {
        Self {
            inner: Arc::new(Mutex::new(SecureLinksManagerInner {
                games,
                links_cache: HashMap::new(),
            })),
            client,
        }
    }
    /// Returns cached [`SecureLinks`] for `game_id` if present and still
    /// valid ([`SecureLinks::is_valid`]); otherwise confirms the account
    /// owns `game_id`, fetches a fresh set, caches it (replacing any expired
    /// entry), and returns that.
    ///
    /// # Errors
    /// [`SecureLinksError::IncorrectGameId`] if `game_id` isn't a valid
    /// integer; [`SecureLinksError::ProductNotOwned`] if the account doesn't
    /// own it; otherwise whatever the owned-games lookup or the fetch itself
    /// failed with.
    pub async fn get_secure_links(&self, game_id: &str) -> Result<SecureLinks, SecureLinksError> {
        {
            let lock = self.inner.lock().await;
            if let Some(links) = lock.links_cache.get(game_id)
                && links.is_valid()
            {
                return Ok(links.clone());
            }
        }

        let available_games = {
            let lock = self.inner.lock().await;
            let games = lock.games.get_owned_games().await?;
            games.owned
        };

        let game_id_i32 = match game_id.parse::<i32>() {
            Ok(id) => id,
            Err(err) => return Err(SecureLinksError::IncorrectGameId(game_id.to_string(), err)),
        };

        if !available_games.contains(&game_id_i32) {
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
    /// Whether `game_id` has a cached, unexpired link set. `false` if
    /// nothing is cached for it yet.
    pub async fn is_valid(&self, game_id: &str) -> bool {
        let lock = self.inner.lock().await;
        lock.links_cache
            .get(game_id)
            .is_some_and(SecureLinks::is_valid)
    }
    /// Evicts `game_id`'s cached link set, if any — the reactive half of
    /// expiry handling: called after a CDN 401 so the next
    /// [`get_secure_links`](Self::get_secure_links) fetches a fresh one (see
    /// `src/downloader/downloader.rs`'s retry loop).
    pub async fn invalidate_secure_links(&self, game_id: &str) {
        let mut lock = self.inner.lock().await;
        lock.links_cache.remove(game_id);
    }
}
