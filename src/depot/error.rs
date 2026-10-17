use thiserror::Error;

use crate::client::ClientError;

/// Errors from depot/build-metadata lookups
/// ([`GogDl::get_product_bundles`](crate::GogDl::get_product_bundles) and
/// friends).
///
/// `#[non_exhaustive]`: a `match` outside the crate needs a wildcard arm.
#[derive(Error, Debug)]
#[non_exhaustive]
pub enum DepotError {
    /// A request failed in transit, was rejected, or didn't decode — see
    /// [`ClientError`](crate::ClientError).
    #[error("Client error: {0}")]
    ClientError(#[from] ClientError),
}
