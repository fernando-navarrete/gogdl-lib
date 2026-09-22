use std::vec::Vec;

use serde::{Deserialize, Serialize};

use crate::games::{GamesError, GamesManager};

pub type ProductId = i32;

/// The product IDs owned by the authenticated account, as returned by
/// [`GogDl::get_owned_products`](crate::GogDl::get_owned_products).
#[derive(Serialize, Deserialize, Clone)]
pub struct OwnedProducts {
    /// The owned product IDs.
    pub owned: Vec<ProductId>,
}
impl OwnedProducts {
    /// An empty `OwnedProducts` — the pre-fetch state, never itself returned by
    /// [`GogDl::get_owned_products`](crate::GogDl::get_owned_products).
    pub fn default() -> Self {
        Self { owned: Vec::new() }
    }
    /// Not reachable from outside the crate — `GamesManager` is not
    /// exported. Call
    /// [`GogDl::get_owned_products`](crate::GogDl::get_owned_products) instead,
    /// which delegates here internally.
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
        let url = format!("https://embed.gog.com/user/data/games");

        let owned_products: OwnedProducts =
            game_manager.client.fetch(&url, false, true, None).await?;

        let mut lock = game_manager.inner.lock().await;
        lock.owned_products = owned_products.clone();
        Ok(owned_products)
    }
}
