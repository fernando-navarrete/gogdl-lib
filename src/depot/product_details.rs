use serde::{Deserialize, Serialize};

use crate::depot::{depot_manager::DepotManager, error::DepotError};

type ProductId = i32;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ProductDetails {
    #[serde(alias = "_embedded")]
    embedded: Embedded,
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

        let mut product_details: ProductDetails =
            download_manager.client.fetch(&url, false, false).await?;

        product_details.title = product_details.embedded.product.title.clone();
        {
            let mut lock = download_manager.inner.lock().await;
            lock.product_details
                .insert(product_id.to_string(), product_details.clone());
        }
        Ok(product_details)
    }
    pub fn get_product_type(&self) -> String {
        self.embedded.product_type.clone()
    }
}
