use serde::{Deserialize, Serialize};

use crate::depot::{depot_manager::DepotManager, error::DepotError};

type ProductId = i32;

/// Store-page details for a product, fetched by
/// [`GogDl::get_product_details`](crate::GogDl::get_product_details).
/// Unlike [`GameDetails`](crate::GameDetails), `product_id` need not be a
/// game — DLC and other pack products work too.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ProductDetails {
    #[serde(alias = "_embedded")]
    embedded: Embedded,
    /// The product's display title.
    #[serde(skip)]
    pub title: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct Embedded {
    #[serde(alias = "productType")]
    product_type: String,
    product: Product,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct Product {
    pub id: ProductId,
    pub title: String,
}

impl ProductDetails {
    /// Not reachable from outside the crate — `DepotManager` is not
    /// exported. Call
    /// [`GogDl::get_product_details`](crate::GogDl::get_product_details)
    /// instead, which delegates here internally.
    pub async fn get_product_details(
        download_manager: &DepotManager,
        product_id: &str,
    ) -> Result<ProductDetails, DepotError> {
        {
            let lock = download_manager.inner.lock().await;
            if let Some(product_details) = lock.product_details.get(product_id) {
                return Ok(product_details.clone());
            }
        }
        let url = format!("https://api.gog.com/v2/games/{}", product_id);

        let mut product_details: ProductDetails = download_manager
            .client
            .fetch(&url, false, false, None)
            .await?;

        product_details.title = product_details.embedded.product.title.clone();
        {
            let mut lock = download_manager.inner.lock().await;
            lock.product_details
                .insert(product_id.to_string(), product_details.clone());
        }
        Ok(product_details)
    }
    /// The product type as reported by GOG (e.g. `"game"`, `"dlc"`, `"pack"`).
    pub fn get_product_type(&self) -> String {
        self.embedded.product_type.clone()
    }

    /// The product ID as reported by GOG.
    pub fn get_product_id(&self) -> ProductId {
        self.embedded.product.id
    }
}
