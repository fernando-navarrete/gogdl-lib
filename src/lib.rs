//! Rust client for GOG's Galaxy backend: authentication, catalog browsing,
//! and downloading/repairing/verifying game installs.
//!
//! [`GogDl`] is the only entry point — construct one with
//! [`GogDl::new_from_client`] and call its methods; every other type in this
//! crate is data you get back or hand in. `GogDl` is not [`Clone`], so share
//! it behind an `Arc` if you need it from more than one task (every method
//! takes `&self`).
//!
//! # Long-running operations
//!
//! [`GogDl::download_game`], [`GogDl::repair_game`] and [`GogDl::verify_files`]
//! report progress by sending events into an
//! [`mpsc::UnboundedSender`](tokio::sync::mpsc::UnboundedSender) you provide,
//! rather than by returning progress from the `async fn` itself. The future
//! only resolves once the whole job is done (or has failed); drain the paired
//! receiver concurrently on another task, or events will pile up unbounded in
//! the channel for the lifetime of the call.
//!
//! `download_game` and [`GogDl::repair_game`] have identical signatures but
//! are not interchangeable — see their doc comments for which one resumes an
//! interrupted install.
//!
//! # Errors
//!
//! Every fallible [`GogDl`] method returns [`GogDlError`], which wraps the
//! per-layer error enum it failed in ([`AuthError`], [`ClientError`],
//! [`DepotError`], [`DownloadError`], [`GamesError`], [`SecureLinksError`] —
//! all re-exported here so you can match on them). Auth failures usually
//! arrive nested, e.g. `GogDlError::ClientError(ClientError::AuthError(_))`,
//! since there is no flattened top-level auth variant.

#![warn(missing_docs)]

mod client;
mod constants;
mod depot;
mod downloader;
mod games;
mod gogdl;
mod secure_links;

pub use client::Auth;
pub use client::TokenObserver;
pub use depot::ProductDetails;
pub use downloader::DownloadEvent;
pub use downloader::DownloadStageEvent;
pub use downloader::DownloadableProduct;
pub use downloader::FileAllocationEvent;
pub use downloader::FileSizeVerificationEvent;
pub use downloader::ProductBundle;
pub use downloader::VerificationEvent;
pub use games::GameBuild;
pub use games::GameBuilds;
pub use games::GameDetails;
pub use games::GameLinks;
pub use games::GameScreenshots;
pub use games::GameSummary;
pub use games::OwnedGames;
pub use gogdl::GogDl;
pub use gogdl::GogDlError;
/// The HTTP client type [`GogDl::new_from_client`] expects. Re-exported so
/// callers don't need a direct `reqwest` dependency just to construct one.
pub use reqwest::Client;

pub use client::AuthError;
pub use client::ClientError;
pub use depot::DepotError;
pub use downloader::DownloadError;
pub use games::GamesError;
pub use secure_links::SecureLinksError;
