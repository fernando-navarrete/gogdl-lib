use std::vec::Vec;

use serde::{Deserialize, Serialize};

use crate::games::{GamesError, GamesManager};

pub type ProductId = i32;

/// Every product ID owned by the authenticated account — games, DLCs, packs
/// and anything else. Crate-internal: the ownership check behind secure
/// links and downloadable products, and the input that
/// [`GogDl::get_owned_games`](crate::GogDl::get_owned_games) filters down to
/// games.
#[derive(Serialize, Deserialize, Clone, Default)]
pub struct OwnedProducts {
    /// The owned product IDs.
    pub owned: Vec<ProductId>,
}
impl OwnedProducts {
    /// Fetches the account's owned product IDs from
    /// `embed.gog.com/user/data/games`. Cached on the `GamesManager` after
    /// the first non-empty result — the network is not re-checked on later
    /// calls.
    pub async fn get_owned_products(
        game_manager: &GamesManager,
    ) -> Result<OwnedProducts, GamesError> {
        let owned_products = {
            let lock = game_manager.inner.lock().await;
            lock.owned_products.clone()
        };
        if !owned_products.owned.is_empty() {
            return Ok(owned_products);
        }
        let url = "https://embed.gog.com/user/data/games".to_string();

        let owned_products: OwnedProducts =
            game_manager.client.fetch(&url, false, true, None).await?;

        let mut lock = game_manager.inner.lock().await;
        lock.owned_products = owned_products.clone();
        Ok(owned_products)
    }
}
