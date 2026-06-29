use serde::{Deserialize, Serialize};

use crate::games::{GamesError, GamesManager};

type ProductId = i32;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ProductDetails {
    #[serde(alias = "_embedded")]
    pub embedded: Embedded,
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
        games_manager: &GamesManager,
        product_id: &str,
    ) -> Result<ProductDetails, GamesError> {
        let url = format!("https://api.gog.com/v2/games/{}", product_id);

        let mut game_details: ProductDetails = match games_manager
            .client
            .get_json::<ProductDetails>(&url)
            .await
            .map_err(GamesError::from)
        {
            Ok(game_details) => game_details,
            Err(err) => {
                return Err(err);
            }
        };
        game_details.title = game_details.embedded.product.title.clone();
        Ok(game_details)
    }
}
