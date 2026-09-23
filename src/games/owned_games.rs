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
    #[serde(alias = "type")]
    produt_type: String,
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

        let owned_games = owned_games
            .iter()
            .filter(|detail| detail.is_ok())
            .map(|detail| detail.as_ref().unwrap())
            .filter(|(_, detail)| detail.produt_type == "game")
            .map(|(id, _)| *id)
            .collect::<Vec<_>>();

        Ok(OwnedGames { owned: owned_games })
    }
}
