use futures_util::{StreamExt, stream};
use serde::{Deserialize, Serialize};

use crate::{
    GamesError,
    games::{GamesManager, gamesdb::is_game_type, owned_products::ProductId},
};

/// The IDs of the owned products that are games, as returned by
/// [`GogDl::get_owned_games`](crate::GogDl::get_owned_games). DLCs, packs
/// and other non-game products the account owns are filtered out.
#[derive(Serialize, Deserialize, Clone)]
pub struct OwnedGames {
    /// The owned game IDs, in no particular order.
    pub owned: Vec<ProductId>,
}

/// Keeps the IDs whose lookup succeeded and came back as a game (see
/// [`is_game_type`]). A failed lookup is dropped silently, whatever the reason
/// (a 404 for a product gamesdb doesn't know, a transport error, a 5xx):
/// `v1.3.0` changes that, so it isn't pinned as a contract. A failure isn't
/// cached, so the next call looks the product up again.
fn keep_games(lookups: &[Result<(ProductId, String), GamesError>]) -> Vec<ProductId> {
    lookups
        .iter()
        .filter_map(|lookup| lookup.as_ref().ok())
        .filter(|(_, product_type)| is_game_type(product_type))
        .map(|(id, _)| *id)
        .collect::<Vec<_>>()
}

impl OwnedGames {
    /// Not reachable from outside the crate — `GamesManager` is not
    /// exported. Call
    /// [`GogDl::get_owned_games`](crate::GogDl::get_owned_games) instead,
    /// which delegates here internally.
    pub async fn get_owned_games(games_manager: &GamesManager) -> Result<OwnedGames, GamesError> {
        let owned_products = games_manager.get_owned_products().await?;

        // `product_type` answers from the cache or fetches and caches on
        // success, never holding the lock across the fetch. Two concurrent
        // calls on a cold cache can both look up the same id; that costs a
        // request, not correctness.
        let lookups = stream::iter(owned_products.owned)
            .map(|id| async move { Ok((id, games_manager.product_type(id).await?)) })
            .buffer_unordered(2)
            .collect::<Vec<Result<(ProductId, String), GamesError>>>()
            .await;

        let owned_games = keep_games(&lookups);

        Ok(OwnedGames { owned: owned_games })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ClientError;
    use crate::client::HttpClient;
    use crate::games::OwnedProducts;
    use crate::test_support::{ChunkServer, Reply, authed_client};

    use crate::games::gamesdb::GamesdbRecord;

    fn product_type(json: &str) -> String {
        serde_json::from_str::<GamesdbRecord>(json)
            .unwrap()
            .product_type
    }

    #[test]
    fn only_products_typed_game_are_kept() {
        let game = product_type(include_str!("../../tests/fixtures/gamesdb_game.json"));
        let dlc = product_type(include_str!("../../tests/fixtures/gamesdb_dlc.json"));
        // GOG's type for a collection like "Alien: Isolation Collection" is
        // "spam", not "pack" (captured from gamesdb).
        let pack = product_type(include_str!("../../tests/fixtures/gamesdb_pack.json"));
        assert_eq!(
            [&game, &dlc, &pack].map(|t| t.as_str()),
            ["game", "dlc", "spam"]
        );

        let lookups = vec![
            Ok((1, dlc)),
            Ok((2, game)),
            Ok((3, pack)),
            Err(GamesError::ClientError(ClientError::AuthError(
                crate::client::AuthError::NotAuthenticated,
            ))),
        ];
        assert_eq!(keep_games(&lookups), vec![2]);
    }

    #[test]
    fn a_failed_lookup_for_a_game_drops_it_silently() {
        // Today's behavior, changed by `v1.3.0` (GAPS "owned games").
        let game = product_type(include_str!("../../tests/fixtures/gamesdb_game.json"));
        let lookups = vec![
            Ok((2, game)),
            Err(GamesError::ProductNotAGame),
            Err(GamesError::ProductNotAGame),
        ];
        assert_eq!(keep_games(&lookups), vec![2]);
    }

    #[tokio::test]
    async fn type_lookups_are_cached_and_failures_retried() {
        let game = include_bytes!("../../tests/fixtures/gamesdb_game.json").to_vec();
        let dlc = include_bytes!("../../tests/fixtures/gamesdb_dlc.json").to_vec();
        let server = ChunkServer::start().await;
        let path = |id: i32| format!("/platforms/gog/external_releases/{id}");
        server.script(&path(1), vec![Reply::Body(game.clone())]);
        server.script(&path(2), vec![Reply::Body(dlc)]);
        server.script(&path(3), vec![Reply::Status(503), Reply::Body(game)]);

        let mut manager = GamesManager::new(HttpClient::new_with_client(reqwest::Client::new()));
        manager.gamesdb_base = server.base_url();
        manager.inner.lock().await.owned_products = Some(OwnedProducts {
            owned: vec![1, 2, 3],
        });

        let first = manager.get_owned_games().await.unwrap();
        assert_eq!(first.owned, vec![1]);
        assert_eq!([1, 2, 3].map(|id| server.requests(&path(id))), [1, 1, 1]);

        let mut second = manager.get_owned_games().await.unwrap().owned;
        second.sort();
        assert_eq!(second, vec![1, 3]);
        assert_eq!([1, 2, 3].map(|id| server.requests(&path(id))), [1, 1, 2]);
    }

    #[tokio::test]
    async fn clear_cache_refetches_the_owned_list_but_keeps_resolved_types() {
        let game = include_bytes!("../../tests/fixtures/gamesdb_game.json").to_vec();
        let server = ChunkServer::start().await;
        let record = |id: i32| format!("/platforms/gog/external_releases/{id}");
        server.script(
            "/user/data/games",
            vec![
                Reply::Body(br#"{"owned": [1]}"#.to_vec()),
                Reply::Body(br#"{"owned": [1, 2]}"#.to_vec()),
            ],
        );
        server.script(&record(1), vec![Reply::Body(game.clone())]);
        server.script(&record(2), vec![Reply::Body(game)]);

        let mut manager = GamesManager::new(authed_client().await);
        manager.gamesdb_base = server.base_url();
        manager.embed_base = server.base_url();

        assert_eq!(manager.get_owned_games().await.unwrap().owned, vec![1]);
        assert_eq!(manager.get_owned_games().await.unwrap().owned, vec![1]);
        assert_eq!(server.requests("/user/data/games"), 1);

        manager.clear_cache().await;
        let mut after = manager.get_owned_games().await.unwrap().owned;
        after.sort();
        assert_eq!(after, vec![1, 2]);
        assert_eq!(server.requests("/user/data/games"), 2);
        assert_eq!([1, 2].map(|id| server.requests(&record(id))), [1, 1]);
    }
}
