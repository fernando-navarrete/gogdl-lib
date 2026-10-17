use std::{collections::HashMap, sync::Arc};

use tokio::sync::{Mutex, OnceCell};

use crate::{
    client::HttpClient,
    games::GamesManager,
    secure_links::{error::SecureLinksError, links::SecureLinks},
};

/// Fetches and caches [`SecureLinks`] per `game_id`, checking ownership on a
/// fresh fetch and re-fetching automatically once a cached entry expires.
///
/// Concurrent callers for one `game_id` share a single ownership check and
/// fetch. A failed fetch is not cached: the callers queued behind it retry
/// one at a time, each returning its own error if it fails too.
#[derive(Clone)]
pub struct SecureLinksManager {
    /// Shared link cache. `pub` for crate-internal reuse; not meant to be
    /// reached from outside the crate. Never held across a fetch.
    pub inner: Arc<Mutex<SecureLinksManagerInner>>,
    /// Used to confirm the account owns `game_id` before fetching its links.
    /// Outside the mutex so the ownership lookup never runs under it.
    pub games: GamesManager,
    /// HTTP client used to fetch fresh secure links.
    pub client: HttpClient,
    /// Test-only: number of [`get_secure_links`](Self::get_secure_links)
    /// calls, to count attempts that never reach the network.
    #[cfg(test)]
    lookups: Arc<std::sync::atomic::AtomicUsize>,
    /// Test-only: number of fetches (fixture or real) made after the
    /// ownership check, to count what concurrent callers collapsed into.
    #[cfg(test)]
    fetches: Arc<std::sync::atomic::AtomicUsize>,
    /// Test-only: how many upcoming fetches fail with
    /// [`SecureLinksError::NoSecureLink`] instead of returning links.
    #[cfg(test)]
    failing_fetches: Arc<std::sync::atomic::AtomicUsize>,
    /// Test-only: links served in place of a fetch from GOG, so eviction
    /// (`invalidate_secure_links`) still leads to a working refetch.
    #[cfg(test)]
    fixture_links: Arc<std::sync::Mutex<HashMap<String, SecureLinks>>>,
}

/// State behind [`SecureLinksManager`]'s mutex: the per-`game_id` link cache.
/// Each entry is a cell the first caller fills while the rest wait on it.
pub struct SecureLinksManagerInner {
    links_cache: HashMap<String, Arc<OnceCell<SecureLinks>>>,
}

impl SecureLinksManager {
    /// Builds a manager with an empty link cache.
    pub fn new(client: HttpClient, games: GamesManager) -> Self {
        Self {
            inner: Arc::new(Mutex::new(SecureLinksManagerInner {
                links_cache: HashMap::new(),
            })),
            games,
            client,
            #[cfg(test)]
            lookups: Arc::default(),
            #[cfg(test)]
            fetches: Arc::default(),
            #[cfg(test)]
            failing_fetches: Arc::default(),
            #[cfg(test)]
            fixture_links: Arc::default(),
        }
    }
    /// Test-only: makes every fetch of `game_id`'s links (after the
    /// ownership check) return `links` instead of calling GOG.
    #[cfg(test)]
    pub fn fixture_links(&self, game_id: &str, links: SecureLinks) {
        self.fixture_links
            .lock()
            .unwrap()
            .insert(game_id.to_string(), links);
    }
    /// Test-only: how many times `get_secure_links` has been called.
    #[cfg(test)]
    pub fn lookups(&self) -> usize {
        self.lookups.load(std::sync::atomic::Ordering::Relaxed)
    }
    /// Test-only: how many fetches ran after the ownership check.
    #[cfg(test)]
    pub fn fetches(&self) -> usize {
        self.fetches.load(std::sync::atomic::Ordering::Relaxed)
    }
    /// Test-only: the next `n` fetches fail with `NoSecureLink`.
    #[cfg(test)]
    pub fn fail_next_fetches(&self, n: usize) {
        self.failing_fetches
            .store(n, std::sync::atomic::Ordering::Relaxed);
    }
    /// Returns cached [`SecureLinks`] for `game_id` if present and still
    /// valid ([`SecureLinks::is_valid`]); otherwise confirms the account
    /// owns `game_id`, fetches a fresh set, caches it (replacing any expired
    /// entry), and returns that.
    ///
    /// Callers that arrive while a fetch for `game_id` is running wait for it
    /// and share its result, so the ownership check and the fetch run once
    /// however many chunks ask. If that fetch fails, nothing is cached and
    /// the waiting callers retry one at a time, so an outage costs one
    /// request at a time, never a burst.
    ///
    /// # Errors
    /// [`SecureLinksError::IncorrectGameId`] if `game_id` isn't a valid
    /// integer; [`SecureLinksError::ProductNotOwned`] if the account doesn't
    /// own it; otherwise whatever the owned-games lookup or the fetch itself
    /// failed with.
    pub async fn get_secure_links(&self, game_id: &str) -> Result<SecureLinks, SecureLinksError> {
        #[cfg(test)]
        self.lookups
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let cell = {
            let mut lock = self.inner.lock().await;
            let cell = lock.links_cache.entry(game_id.to_string()).or_default();
            if cell.get().is_some_and(|links| !links.is_valid()) {
                // Expired: the first caller past the deadline swaps in an
                // empty cell and the rest join it.
                *cell = Arc::new(OnceCell::new());
            }
            cell.clone()
        };
        cell.get_or_try_init(|| self.fetch(game_id)).await.cloned()
    }
    /// The ownership check and the fetch behind one cache cell.
    async fn fetch(&self, game_id: &str) -> Result<SecureLinks, SecureLinksError> {
        let available_games = self.games.get_owned_products().await?.owned;

        let game_id_i32 = match game_id.parse::<i32>() {
            Ok(id) => id,
            Err(err) => return Err(SecureLinksError::IncorrectGameId(game_id.to_string(), err)),
        };

        if !available_games.contains(&game_id_i32) {
            return Err(SecureLinksError::ProductNotOwned(game_id.to_string()));
        }
        #[cfg(test)]
        {
            use std::sync::atomic::Ordering::Relaxed;
            self.fetches.fetch_add(1, Relaxed);
            if self
                .failing_fetches
                .try_update(Relaxed, Relaxed, |n| n.checked_sub(1))
                .is_ok()
            {
                return Err(SecureLinksError::NoSecureLink);
            }
        }
        #[cfg(test)]
        let fixture = self.fixture_links.lock().unwrap().get(game_id).cloned();
        #[cfg(not(test))]
        let fixture: Option<SecureLinks> = None;
        match fixture {
            Some(links) => Ok(links),
            None => SecureLinks::get_secure_links(self, game_id).await,
        }
    }
    /// Whether `game_id` has a cached, unexpired link set. `false` if
    /// nothing is cached for it yet, or a fetch for it is still running.
    #[cfg(test)]
    pub async fn is_valid(&self, game_id: &str) -> bool {
        let lock = self.inner.lock().await;
        lock.links_cache
            .get(game_id)
            .and_then(|cell| cell.get())
            .is_some_and(SecureLinks::is_valid)
    }
    /// Evicts `game_id`'s cached link set if it is still `stale`, the set the
    /// caller was using — the reactive half of expiry handling: called after
    /// a CDN 401 so the next [`get_secure_links`](Self::get_secure_links)
    /// fetches a fresh one (see `src/downloader/engine.rs`'s retry loop).
    ///
    /// Comparing against `stale` keeps a late caller from evicting the fresh
    /// set a sibling has just fetched, which would restart the storm
    /// [`get_secure_links`](Self::get_secure_links) collapses. A fetch still
    /// running for `game_id` is left alone.
    pub async fn invalidate_secure_links(&self, game_id: &str, stale: &SecureLinks) {
        let mut lock = self.inner.lock().await;
        if lock
            .links_cache
            .get(game_id)
            .and_then(|cell| cell.get())
            .is_some_and(|cached| cached == stale)
        {
            lock.links_cache.remove(game_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Duration};

    use super::*;
    use crate::games::OwnedProducts;
    use crate::secure_links::links::{CdnUrlParams, UrlFormat};

    fn links() -> SecureLinks {
        links_with("", None)
    }

    fn links_with(token: &str, expires_at: Option<u64>) -> SecureLinks {
        SecureLinks {
            product_id: 1,
            urls: vec![UrlFormat {
                endpoint_name: "test".into(),
                url_format: "{base_url}{path}".into(),
                priority: 1,
                parameters: CdnUrlParams {
                    base_url: "http://127.0.0.1".into(),
                    path: "/depot".into(),
                    token: token.into(),
                    expires_at,
                    dirs: None,
                    ttl: None,
                    source: None,
                    gog_token: None,
                    l: None,
                },
            }],
        }
    }

    /// An ownership check stuck inside the games manager (its `inner` is held
    /// here, the await the manager's own lock used to span) must not block a
    /// cache hit for another game.
    #[tokio::test(start_paused = true)]
    async fn an_ownership_check_does_not_block_a_cache_hit() {
        let client = HttpClient::new_with_client(reqwest::Client::new());
        let games = GamesManager::new(client.clone());
        games.inner.lock().await.owned_products = Some(OwnedProducts { owned: vec![2] });
        let manager = Arc::new(SecureLinksManager::new(client, games.clone()));
        manager.fixture_links("2", links());
        manager.get_secure_links("2").await.unwrap();
        // Force a cold lookup of game 1 to need the owned list again.
        games.inner.lock().await.owned_products = None;

        let games_guard = games.inner.lock().await;
        let cold = tokio::spawn({
            let manager = manager.clone();
            async move { manager.get_secure_links("1").await }
        });
        tokio::time::sleep(Duration::from_millis(1)).await;

        tokio::time::timeout(Duration::from_secs(1), manager.get_secure_links("2"))
            .await
            .expect("a cache hit waited on another game's ownership check")
            .unwrap();

        drop(games_guard);
        cold.abort();
    }

    const CALLERS: usize = 8;

    /// A manager that owns game 1, with `links` as its fixture.
    async fn manager_for_game_1(links: SecureLinks) -> (Arc<SecureLinksManager>, GamesManager) {
        let client = HttpClient::new_with_client(reqwest::Client::new());
        let games = GamesManager::new(client.clone());
        games.inner.lock().await.owned_products = Some(OwnedProducts { owned: vec![1] });
        let manager = Arc::new(SecureLinksManager::new(client, games.clone()));
        manager.fixture_links("1", links);
        (manager, games)
    }

    /// Starts `CALLERS` concurrent lookups of game 1 and lets them all pile
    /// up behind the ownership check (`games.inner` is held meanwhile).
    async fn race(
        manager: &Arc<SecureLinksManager>,
        games: &GamesManager,
    ) -> Vec<Result<SecureLinks, SecureLinksError>> {
        let guard = games.inner.lock().await;
        let tasks: Vec<_> = (0..CALLERS)
            .map(|_| {
                let manager = manager.clone();
                tokio::spawn(async move { manager.get_secure_links("1").await })
            })
            .collect();
        tokio::time::sleep(Duration::from_millis(1)).await;
        drop(guard);
        let mut out = Vec::new();
        for task in tasks {
            out.push(task.await.unwrap());
        }
        out
    }

    #[tokio::test(start_paused = true)]
    async fn concurrent_cold_lookups_share_one_fetch() {
        let (manager, games) = manager_for_game_1(links()).await;

        let results = race(&manager, &games).await;

        assert!(results.iter().all(Result::is_ok));
        assert_eq!(manager.fetches(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn concurrent_lookups_past_an_expired_entry_share_one_fetch() {
        let (manager, games) = manager_for_game_1(links_with("old", Some(1))).await;
        manager.get_secure_links("1").await.unwrap();
        assert_eq!(manager.fetches(), 1);
        manager.fixture_links("1", links_with("new", None));

        let results = race(&manager, &games).await;

        assert!(results.iter().all(Result::is_ok));
        assert_eq!(manager.fetches(), 2);
        assert_eq!(
            manager.get_secure_links("1").await.unwrap(),
            links_with("new", None)
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_failed_fetch_is_retried_by_the_next_waiter_only() {
        let (manager, games) = manager_for_game_1(links()).await;
        manager.fail_next_fetches(1);

        let results = race(&manager, &games).await;

        assert_eq!(results.iter().filter(|r| r.is_err()).count(), 1);
        assert_eq!(manager.fetches(), 2, "one failed fetch, one retry");
    }

    #[tokio::test(start_paused = true)]
    async fn a_failed_fetch_is_not_cached() {
        let (manager, _games) = manager_for_game_1(links()).await;
        manager.fail_next_fetches(1);

        assert!(manager.get_secure_links("1").await.is_err());
        assert!(!manager.is_valid("1").await);
        manager.get_secure_links("1").await.unwrap();

        assert_eq!(manager.fetches(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn invalidating_a_stale_set_keeps_a_fresher_one() {
        let (manager, _games) = manager_for_game_1(links_with("a", None)).await;
        let a = manager.get_secure_links("1").await.unwrap();
        manager.fixture_links("1", links_with("b", None));

        manager.invalidate_secure_links("1", &a).await;
        let b = manager.get_secure_links("1").await.unwrap();
        assert_eq!(b, links_with("b", None));
        assert_eq!(manager.fetches(), 2);

        // A sibling still holding `a` evicts late: `b` must survive.
        manager.invalidate_secure_links("1", &a).await;
        assert_eq!(manager.get_secure_links("1").await.unwrap(), b);
        assert_eq!(manager.fetches(), 2);
    }

    /// A game bought after the owned list was cached passes the ownership
    /// check once `clear_cache` has dropped the list.
    #[tokio::test]
    async fn a_new_purchase_passes_the_ownership_check_after_a_clear() {
        use crate::test_support::{ChunkServer, Reply, authed_client};
        let server = ChunkServer::start().await;
        server.script(
            "/user/data/games",
            vec![Reply::Body(br#"{"owned": [1, 2]}"#.to_vec())],
        );
        let client = authed_client().await;
        let mut games = GamesManager::new(client.clone());
        games.embed_base = server.base_url();
        games.inner.lock().await.owned_products = Some(OwnedProducts { owned: vec![1] });
        let manager = SecureLinksManager::new(client, games.clone());
        manager.fixture_links("2", links());

        assert!(matches!(
            manager.get_secure_links("2").await,
            Err(SecureLinksError::ProductNotOwned(_))
        ));
        games.clear_cache().await;
        manager.get_secure_links("2").await.unwrap();
    }
}
