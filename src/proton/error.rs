use thiserror::Error;

use crate::ClientError;

/// Errors from listing Proton-GE releases
/// ([`GogDl::get_proton_releases`](crate::GogDl::get_proton_releases)).
///
/// Unlike the other per-layer error enums in this crate, every variant here
/// is currently reachable — the GitHub call has no separate decode/auth path
/// of its own, so any failure (network, a non-2xx status, or a JSON decode
/// error) arrives as [`ClientError`].
#[derive(Debug, Error)]
pub enum ProtonError {
    /// The underlying HTTP request to GitHub failed — see [`ClientError`].
    /// Notably includes a 403 from a missing `User-Agent` or an exhausted
    /// rate limit; see
    /// [`GogDl::get_proton_releases`](crate::GogDl::get_proton_releases).
    #[error("Client error: {0}")]
    ClientError(#[from] ClientError),
}
