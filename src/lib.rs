//! Rust client for GOG's Galaxy backend: authentication, catalog browsing,
//! downloading/repairing/verifying game installs, syncing cloud saves, and
//! listing Proton-GE releases.
//!
//! [`GogDl`] is the only entry point — construct one with
//! [`GogDl::new_from_client`] and call its methods; every other type in this
//! crate is data you get back or hand in. `GogDl` is not [`Clone`], so share
//! it behind an `Arc` if you need it from more than one task (every method
//! takes `&self`).
//!
//! # Long-running operations
//!
//! [`GogDl::download_game`], [`GogDl::repair_game`], [`GogDl::verify_files`]
//! and [`GogDl::download_proton_release`] report progress by sending events
//! into an [`mpsc::UnboundedSender`](tokio::sync::mpsc::UnboundedSender) you
//! provide, rather than by returning progress from the `async fn` itself. The
//! future only resolves once the whole job is done (or has failed); drain the
//! paired receiver concurrently on another task, or events will pile up
//! unbounded in the channel for the lifetime of the call.
//!
//! `download_game` and [`GogDl::repair_game`] have identical signatures but
//! are not interchangeable — see their doc comments for which one resumes an
//! interrupted install.
//!
//! # Errors
//!
//! Every fallible [`GogDl`] method returns [`GogDlError`], which wraps the
//! per-layer error enum it failed in ([`AuthError`], [`ClientError`],
//! [`DepotError`], [`DownloadError`], [`GamesError`], [`SecureLinksError`],
//! [`ProtonError`] — all re-exported here so you can match on them). Auth
//! failures usually arrive nested, e.g.
//! `GogDlError::ClientError(ClientError::AuthError(_))`, since there is no
//! flattened top-level auth variant.
//!
//! # Proton-GE releases
//!
//! [`GogDl::get_proton_releases`] is the one method that doesn't talk to GOG
//! at all — it lists releases from the `proton-ge-custom` GitHub repo, with
//! no GOG auth and no caching. GitHub rejects unauthenticated requests that
//! carry no `User-Agent` header with a 403, so the `reqwest::Client` passed
//! to [`GogDl::new_from_client`] must set one for this call to succeed. See
//! [`GogDl::get_proton_releases`] for details.

#![warn(missing_docs)]

mod client;
mod constants;
mod depot;
mod downloader;
mod fs;
mod games;
mod gogdl;
mod proton;
mod saves;
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
pub use proton::GithubAsset;
pub use proton::ProtonDownloadEvent;
pub use proton::ProtonGeRelease;
pub use proton::ProtonGeReleasesPage;
/// The HTTP client type [`GogDl::new_from_client`] expects. Re-exported so
/// callers don't need a direct `reqwest` dependency just to construct one.
pub use reqwest::Client;

pub use client::AuthError;
pub use client::ClientError;
pub use depot::DepotError;
pub use downloader::DownloadError;
pub use games::GamesError;
pub use proton::ProtonError;
pub use secure_links::SecureLinksError;

pub use saves::CloudStorageLocation;
pub use saves::RemoteConfig;
pub use saves::SaveFile;
pub use saves::SavesDownloadEvent;
pub use saves::SavesError;
pub use saves::SavesUploadEvent;
