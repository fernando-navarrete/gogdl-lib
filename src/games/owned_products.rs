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
    /// the first successful fetch (an empty list included) until
    /// `clear_cache` drops it.
    pub async fn get_owned_products(
        game_manager: &GamesManager,
    ) -> Result<OwnedProducts, GamesError> {
        {
            let lock = game_manager.inner.lock().await;
            if let Some(owned_products) = &lock.owned_products {
                return Ok(owned_products.clone());
            }
        }
        let url = game_manager.owned_products_url();

        let owned_products: OwnedProducts =
            game_manager.client.fetch(&url, false, true, None).await?;

        let mut lock = game_manager.inner.lock().await;
        lock.owned_products = Some(owned_products.clone());
        Ok(owned_products)
    }
}
