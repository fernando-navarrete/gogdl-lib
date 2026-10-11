use futures_util::{StreamExt, stream};
use serde::{Deserialize, Serialize};

use crate::{
    GamesError,
    games::{GamesManager, owned_products::ProductId},
};

/// The IDs of the owned products that are games, as returned by
/// [`GogDl::get_owned_games`](crate::GogDl::get_owned_games). DLCs, packs
/// and other non-game products the account owns are filtered out.
#[derive(Serialize, Deserialize, Clone)]
pub struct OwnedGames {
    /// The owned game IDs, in no particular order.
    pub owned: Vec<ProductId>,
}

#[derive(Deserialize)]
struct GogdbDetails {
    #[serde(rename = "type")]
    product_type: String,
}

/// Keeps the IDs whose lookup succeeded and came back as `type == "game"`.
/// A failed lookup is dropped silently, whatever the reason (a 404 for a
/// product gamesdb doesn't know, a transport error, a 5xx): `v1.3.0` changes
/// that, so it isn't pinned as a contract. A failure isn't cached, so the
/// next call looks the product up again.
fn keep_games(lookups: &[Result<(ProductId, GogdbDetails), GamesError>]) -> Vec<ProductId> {
    lookups
        .iter()
        .filter_map(|detail| detail.as_ref().ok())
        .filter(|(_, detail)| detail.product_type == "game")
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

        // The lock is held only to split the ids and to store the results,
        // never across a lookup. Two concurrent calls on a cold cache can
        // both look up the same id; that costs a request, not correctness.
        let (cached, missing): (Vec<_>, Vec<_>) = {
            let lock = games_manager.inner.lock().await;
            owned_products
                .owned
                .iter()
                .partition(|id| lock.product_types.contains_key(id))
        };
        let mut lookups: Vec<Result<(ProductId, GogdbDetails), GamesError>> = {
            let lock = games_manager.inner.lock().await;
            cached
                .into_iter()
                .map(|id| {
                    let product_type = lock.product_types[id].clone();
                    Ok((*id, GogdbDetails { product_type }))
                })
                .collect()
        };

        let fetched = stream::iter(missing)
            .map(|&id| async move {
                let url = games_manager.gamesdb_release_url(id);
                let details: GogdbDetails =
                    games_manager.client.fetch(&url, false, false, None).await?;
                Ok::<(i32, GogdbDetails), GamesError>((id, details))
            })
            .buffer_unordered(2)
            .collect::<Vec<_>>()
            .await;

        {
            let mut lock = games_manager.inner.lock().await;
            for (id, details) in fetched.iter().flatten() {
                lock.product_types.insert(*id, details.product_type.clone());
            }
        }
        lookups.extend(fetched);

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

    fn details(json: &str) -> GogdbDetails {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn only_products_typed_game_are_kept() {
        let game = details(include_str!("../../tests/fixtures/gamesdb_game.json"));
        let dlc = details(include_str!("../../tests/fixtures/gamesdb_dlc.json"));
        // GOG's type for a collection like "Alien: Isolation Collection" is
        // "spam", not "pack" (captured from gamesdb).
        let pack = details(include_str!("../../tests/fixtures/gamesdb_pack.json"));
        assert_eq!(
            [&game, &dlc, &pack].map(|d| d.product_type.as_str()),
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
        let game = details(include_str!("../../tests/fixtures/gamesdb_game.json"));
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
