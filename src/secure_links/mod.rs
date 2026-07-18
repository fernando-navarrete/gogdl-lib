mod error;
mod links_manager;
mod secure_links;

pub use error::SecureLinksError;
pub use links_manager::SecureLinksManager;
pub use secure_links::SecureLinks;
// Crate-internal only: lets the downloader name the endpoint type in
// signatures (e.g. `stages::download_unit_with_retries`) without adding
// `UrlFormat` to the crate's curated public surface (see api.md §9).
pub(crate) use secure_links::UrlFormat;
