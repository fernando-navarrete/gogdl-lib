mod build_metadata;
mod depot_info;
mod depot_manager;
mod error;
mod product_details;

pub use build_metadata::Depot;
pub use depot_info::{Chunk, DepotFile};
pub use depot_manager::DepotManager;
pub use error::DepotError;
pub use product_details::ProductDetails;
