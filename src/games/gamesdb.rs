use serde::Deserialize;

use crate::games::{GamesError, GamesManager, owned_products::ProductId};

/// What `gamesdb` calls a game in its `type` field. `gamesdb.gog.com` is not a
/// documented GOG API; the values seen in captures of
/// `/platforms/gog/external_releases/{id}` are `game`, `dlc` and `spam` (a
/// pack). It is the only place in the crate this string is compared.
const GAMESDB_GAME_TYPE: &str = "game";

/// Whether a `gamesdb` `type` names a game. Any other spelling, including a
/// type GOG adds later, counts as a non-game.
pub fn is_game_type(product_type: &str) -> bool {
    product_type == GAMESDB_GAME_TYPE
}

/// The one field of a `gamesdb` record the crate reads for classification.
#[derive(Deserialize)]
pub struct GamesdbRecord {
    #[serde(rename = "type")]
    pub product_type: String,
}

impl GamesManager {
    /// The `gamesdb` `type` of `id`, from the cache or one unauthenticated
    /// fetch. Only a successful lookup is cached, so a failure is retried by
    /// the next call. The lock is never held across the fetch.
    pub async fn product_type(&self, id: ProductId) -> Result<String, GamesError> {
        if let Some(product_type) = self.inner.lock().await.product_types.get(&id) {
            return Ok(product_type.clone());
        }
        let url = self.gamesdb_release_url(id);
        let record: GamesdbRecord = self.client.fetch(&url, false, false, None).await?;
        self.inner
            .lock()
            .await
            .product_types
            .insert(id, record.product_type.clone());
        Ok(record.product_type)
    }
}
