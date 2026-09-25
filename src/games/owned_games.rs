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
/// that, so it isn't pinned as a contract.
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

        let owned_games = stream::iter(owned_products.owned)
            .map(|id| async move {
                let url = format!(
                    "https://gamesdb.gog.com/platforms/gog/external_releases/{}",
                    id
                );
                let details: GogdbDetails =
                    games_manager.client.fetch(&url, false, false, None).await?;
                Ok::<(i32, GogdbDetails), GamesError>((id, details))
            })
            .buffer_unordered(2)
            .collect::<Vec<_>>()
            .await;

        let owned_games = keep_games(&owned_games);

        Ok(OwnedGames { owned: owned_games })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ClientError;

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
}
