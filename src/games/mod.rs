mod error;
mod game_build;
mod game_details;
mod games;
mod owned_games;
mod product_details;

pub use error::GamesError;
pub use game_build::GameBuilds;
pub use game_details::GameDetails;
pub use games::GamesManager;
pub use owned_games::OwnedGames;
pub use product_details::ProductDetails;
