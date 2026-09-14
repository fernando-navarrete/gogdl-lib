use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::mpsc;

use crate::DownloadStageEvent;
use crate::client::{HttpClient, TokenObserver};
use crate::depot::{DepotManager, ProductDetails};
use crate::downloader::{DownloadManager, DownloadableProduct, ProductBundle, VerificationEvent};
use crate::games::{
    GameBuilds, GameDetails, GameLinks, GameScreenshots, GameSummary, GamesManager, OwnedGames,
};
use crate::gogdl::error::GogDlError;
use crate::proton::{ProtonDownloadEvent, ProtonGeRelease, ProtonGeReleasesPage, ProtonManager};
use crate::saves::{SavesAuth, SavesError, SavesManager};
use crate::secure_links::SecureLinksManager;

/// The single entry point to this crate. Every operation — auth, catalog
/// browsing, downloading — is a method on `GogDl`; the managers it holds
/// internally are private implementation detail.
///
/// Not [`Clone`]. Every method takes `&self`, so wrap it in an `Arc` to share
/// it across tasks rather than cloning it.
pub struct GogDl {
    games: GamesManager,
    depot: DepotManager,
    downloader: DownloadManager,
    proton: ProtonManager,
    saves: SavesManager,
    client: HttpClient,
}

impl GogDl {
    /// Builds a `GogDl` around a caller-supplied `reqwest::Client`. No
    /// network calls are made and no auth state exists yet — call
    /// [`restore_auth`](Self::restore_auth) or
    /// [`login_with_code`](Self::login_with_code) before anything that needs
    /// authentication.
    pub fn new_from_client(client: reqwest::Client) -> Self {
        let http_client = HttpClient::new_with_client(client);
        let games_manager = GamesManager::new(http_client.clone());
        let depot_manager = DepotManager::new(http_client.clone());
        let secure_links_manager =
            SecureLinksManager::new(http_client.clone(), games_manager.clone());
        let download_manager = DownloadManager::new(
            depot_manager.clone(),
            secure_links_manager.clone(),
            games_manager.clone(),
            http_client.clone(),
        );

        let proton_manager = ProtonManager::new(http_client.clone());
        let saves_manager = SavesManager::new(
            http_client.clone(),
            depot_manager.clone(),
            games_manager.clone(),
        );
        Self {
            games: games_manager,
            depot: depot_manager,
            downloader: download_manager,
            proton: proton_manager,
            saves: saves_manager,
            client: http_client,
        }
    }

    /// Lists releases of [Proton-GE](https://github.com/GloriousEggroll/proton-ge-custom),
    /// `page` pages of `per_page` releases at a time (`page` is 1-indexed;
    /// GitHub caps `per_page` at 100). Unlike every other method on `GogDl`,
    /// this talks to `api.github.com`, not GOG — no GOG auth is used or
    /// required.
    ///
    /// **Not cached** — every call is a fresh round-trip.
    ///
    /// **Requires a `User-Agent` on the underlying client.** GitHub answers
    /// 403 to any request with no `User-Agent` header; the `reqwest::Client`
    /// passed to [`new_from_client`](Self::new_from_client) must set one
    /// (e.g. via [`reqwest::ClientBuilder::user_agent`]) for this call to
    /// succeed. The unauthenticated rate limit is also 60 requests/hour per
    /// IP, shared with everything else on that address.
    ///
    /// # Errors
    /// [`GogDlError::ProtonError`] wrapping
    /// [`crate::ProtonError::ClientError`], notably a
    /// [`crate::ClientError::HttpError`] with `status: 403` for a missing
    /// `User-Agent` or an exhausted rate limit — returned immediately, since
    /// the underlying fetch only retries a 401 or a transport error, and a
    /// 403 is neither.
    pub async fn get_proton_releases(
        &self,
        page: u32,
        per_page: u32,
    ) -> Result<ProtonGeReleasesPage, GogDlError> {
        let proton_releases = self.proton.get_releases_page(page, per_page).await?;
        Ok(proton_releases)
    }

    /// Fetches a single [Proton-GE](https://github.com/GloriousEggroll/proton-ge-custom)
    /// release by its `tag` (e.g. `"GE-Proton11-6"`), rather than a whole page
    /// as [`get_proton_releases`](Self::get_proton_releases) does. Like that
    /// method, this talks to `api.github.com`, not GOG — no GOG auth is used
    /// or required.
    ///
    /// **Not cached** — every call is a fresh round-trip.
    ///
    /// **Requires a `User-Agent` on the underlying client**, for the same
    /// reason as [`get_proton_releases`](Self::get_proton_releases) — GitHub
    /// answers 403 to any request with no `User-Agent` header. Subject to the
    /// same unauthenticated rate limit (60 requests/hour per IP).
    ///
    /// # Errors
    /// [`GogDlError::ProtonError`] wrapping
    /// [`crate::ProtonError::ClientError`], notably a
    /// [`crate::ClientError::HttpError`] with `status: 404` if `tag` doesn't
    /// match any release, or `status: 403` for a missing `User-Agent` or an
    /// exhausted rate limit.
    pub async fn get_proton_release_by_tag(
        &self,
        tag: &str,
    ) -> Result<ProtonGeRelease, GogDlError> {
        let release = self.proton.get_release_by_tag(tag).await?;
        Ok(release)
    }

    /// Downloads and extracts one [Proton-GE](https://github.com/GloriousEggroll/proton-ge-custom)
    /// release, as returned by [`get_proton_releases`](Self::get_proton_releases).
    ///
    /// The release's tarball is never written to disk as a `.tar.gz` — the
    /// network response is decompressed and extracted directly as it
    /// arrives, bounded by a fixed-size in-memory buffer regardless of the
    /// tarball's size. `path` is the *parent* directory the release's own
    /// top-level directory is extracted into (e.g. `GE-Proton11-6/`) and is
    /// created (and canonicalized) if missing; the returned [`PathBuf`] is
    /// that extracted directory.
    ///
    /// The transfer is gated on the destination disk having at least the
    /// tarball's compressed size free before a single byte is read — a
    /// lower bound only, since the extracted tree is considerably larger
    /// than the tarball.
    ///
    /// Reports progress on `tx` as [`ProtonDownloadEvent`]: one `Downloading`
    /// with the compressed total size, then a `Progress` delta per network
    /// read, interleaved with an `Extracted` event per file/directory/symlink
    /// written to disk. Resolves only once the whole operation finishes or
    /// fails — drain `tx`'s paired receiver concurrently on another task.
    ///
    /// **Not resumable** — there is no retry loop here, unlike
    /// [`download_game`](Self::download_game). A failure partway through
    /// leaves a partial tree under `path` that you're responsible for
    /// removing before trying again.
    ///
    /// # Errors
    /// [`GogDlError::ProtonError`], notably
    /// [`crate::ProtonError::NoSuitableAsset`] if `release` has no Linux
    /// x86_64 tarball, [`crate::ProtonError::NotEnoughFreeSpace`] or
    /// [`crate::ProtonError::CouldNotResolveFreeSpace`] if the pre-flight
    /// space check fails, or [`crate::ProtonError::Io`] for a network, disk,
    /// or extraction failure.
    pub async fn download_proton_release(
        &self,
        release: &ProtonGeRelease,
        path: &Path,
        tx: mpsc::UnboundedSender<ProtonDownloadEvent>,
    ) -> Result<PathBuf, GogDlError> {
        let extracted_path = self
            .proton
            .download_proton_release(release, path, tx)
            .await?;
        Ok(extracted_path)
    }

    /// Restores a previously-persisted auth state, as returned by
    /// [`login_with_code`](Self::login_with_code) or captured via a
    /// [`TokenObserver`]. Does not touch the network.
    ///
    /// # Errors
    /// [`GogDlError::ClientError`] wrapping [`crate::AuthError::AuthDecodeError`] if
    /// `json_str` isn't a valid serialized [`crate::Auth`].
    pub async fn restore_auth(&self, json_str: &str) -> Result<(), GogDlError> {
        self.client.restore_auth_from_string(json_str).await?;
        Ok(())
    }
    /// The GOG login page URL to send a user to. Exchange the `code` it
    /// redirects back with for a session via
    /// [`login_with_code`](Self::login_with_code).
    pub fn get_login_url(&self) -> &str {
        self.client.get_login_url()
    }
    /// Exchanges an authorization code (from the redirect after
    /// [`get_login_url`](Self::get_login_url)) for a session, and returns the
    /// serialized [`crate::Auth`] to persist — hand it back to
    /// [`restore_auth`](Self::restore_auth) on the next run.
    ///
    /// # Errors
    /// [`GogDlError::ClientError`] on a rejected code or a transport failure.
    pub async fn login_with_code(&self, code: &str) -> Result<String, GogDlError> {
        let auth_string = self.client.login_with_code(code).await?;
        Ok(auth_string)
    }
    /// The IDs of every product the authenticated account owns. Cached
    /// in-process for the lifetime of this `GogDl` after the first
    /// successful call — the network is not re-checked on later calls.
    ///
    /// # Errors
    /// [`GogDlError::GameError`] wrapping [`crate::GamesError::ClientError`] on
    /// auth or transport failure.
    pub async fn get_owned_games(&self) -> Result<OwnedGames, GogDlError> {
        let owned_games = self.games.get_owned_games().await?;
        Ok(owned_games)
    }
    /// Details for a single game. Cached per `game_id` after the first call.
    ///
    /// # Errors
    /// [`GogDlError::GameError`] wrapping [`crate::GamesError::ProductNotAGame`] if
    /// `game_id` names a non-game product (e.g. a DLC), or
    /// [`crate::GamesError::ClientError`] on transport failure.
    pub async fn get_game_details(&self, game_id: i32) -> Result<GameDetails, GogDlError> {
        let game_details = self.games.get_game_details(game_id).await?;
        Ok(game_details)
    }
    /// The published builds for a game, newest and oldest alike. Always
    /// queries the Windows depot (`os/windows/builds`) — there is currently
    /// no way to request a different OS. Cached per `game_id`.
    ///
    /// # Errors
    /// [`GogDlError::GameError`] wrapping [`crate::GamesError::ClientError`] on
    /// transport failure.
    pub async fn get_game_builds(&self, game_id: i32) -> Result<GameBuilds, GogDlError> {
        let game_builds = self.games.get_game_builds(game_id).await?;
        Ok(game_builds)
    }
    /// Box art, background and Galaxy background image links for a game.
    /// Cached per `game_id`.
    ///
    /// # Errors
    /// [`GogDlError::GameError`] wrapping [`crate::GamesError::ClientError`] on
    /// transport failure.
    pub async fn get_game_links(&self, game_id: i32) -> Result<GameLinks, GogDlError> {
        let game_links = self.games.get_game_links(game_id).await?;
        Ok(game_links)
    }
    /// The store summary/description text for a game. Cached per `game_id`.
    ///
    /// # Errors
    /// [`GogDlError::GameError`] wrapping [`crate::GamesError::ClientError`] on
    /// transport failure.
    pub async fn get_game_summary(&self, game_id: i32) -> Result<GameSummary, GogDlError> {
        let game_summary = self.games.get_game_summary(game_id).await?;
        Ok(game_summary)
    }
    /// Screenshot links for a game — resolve them to concrete URLs with
    /// [`GameScreenshots::resolve_links`]. Cached per `game_id`.
    ///
    /// # Errors
    /// [`GogDlError::GameError`] wrapping [`crate::GamesError::ClientError`] on
    /// transport failure.
    pub async fn get_game_screenshots(&self, game_id: i32) -> Result<GameScreenshots, GogDlError> {
        let game_screenshots = self.games.get_game_screenshots(game_id).await?;
        Ok(game_screenshots)
    }
    /// Store-page details (title, product type) for a product, which — unlike
    /// [`get_game_details`](Self::get_game_details) — need not be a game
    /// (DLC and packs work too). `product_id` is the product ID as a string.
    /// Cached per `product_id`.
    ///
    /// # Errors
    /// [`GogDlError::DepotError`] wrapping [`crate::DepotError::ClientError`] on
    /// transport failure.
    pub async fn get_product_details(
        &self,
        product_id: &str,
    ) -> Result<ProductDetails, GogDlError> {
        let product_details = self.depot.get_product_details(product_id).await?;
        Ok(product_details)
    }
    /// The sub-products (base game, DLCs, ...) available in `build_name` of
    /// `game_id` that the authenticated account owns, each with its raw
    /// depot list (an internal type, not exported). Use this to build the
    /// `selected_products` list for
    /// [`get_product_bundles`](Self::get_product_bundles). Cached per
    /// `(game_id, build_name)`.
    ///
    /// # Errors
    /// [`GogDlError::DownloadError`] wrapping [`crate::DownloadError::BuildNotFound`]
    /// if no build named `build_name` exists.
    pub async fn get_downloadable_products(
        &self,
        game_id: i32,
        build_name: &str,
    ) -> Result<Vec<DownloadableProduct>, GogDlError> {
        let downloadable_products = self
            .downloader
            .get_downloadable_products(game_id, build_name)
            .await?;
        Ok(downloadable_products)
    }
    /// Resolves `selected_products` (product IDs from
    /// [`get_downloadable_products`](Self::get_downloadable_products)) into
    /// the concrete [`ProductBundle`]s — depot manifests fetched from the
    /// CDN — that [`verify_files`](Self::verify_files),
    /// [`download_game`](Self::download_game) and
    /// [`repair_game`](Self::repair_game) consume.
    ///
    /// **Not cached** — every call re-fetches every selected depot's
    /// manifest from the CDN, even if an earlier call already resolved the
    /// same `(game_id, build_name)`. Depot manifests are typically the
    /// largest payloads this crate fetches; call this once and hold onto the
    /// result rather than re-deriving it.
    ///
    /// # Errors
    /// [`GogDlError::DownloadError`] wrapping [`crate::DownloadError::BuildNotFound`]
    /// if no build named `build_name` exists.
    pub async fn get_product_bundles(
        &self,
        game_id: i32,
        build_name: &str,
        selected_products: &[i32],
    ) -> Result<Vec<ProductBundle>, GogDlError> {
        let downloadable_files = self
            .downloader
            .get_downloadable_files(game_id, build_name, selected_products)
            .await?;
        Ok(downloadable_files)
    }
    /// Checksums every chunk of `bundles` already on disk under `path` and
    /// reports the result of each on `tx`, without downloading anything.
    ///
    /// Like [`download_game`](Self::download_game) and
    /// [`repair_game`](Self::repair_game), this resolves only once every
    /// unit has been checked — drain `tx`'s paired receiver concurrently on
    /// another task, since events accumulate in the unbounded channel
    /// otherwise.
    ///
    /// # Errors
    /// [`GogDlError::DownloadError`] wrapping
    /// [`crate::DownloadError::ChunkIntegrityCheckFailed`] with the count of units
    /// that failed verification (missing, unreadable, or checksum mismatch).
    pub async fn verify_files(
        &self,
        bundles: Vec<ProductBundle>,
        path: &str,
        tx: mpsc::UnboundedSender<VerificationEvent>,
    ) -> Result<(), GogDlError> {
        self.downloader.verify_download(bundles, path, tx).await?;
        Ok(())
    }
    /// Downloads every chunk of `bundles` into `path` from scratch.
    ///
    /// **This does not resume.** Unlike [`repair_game`](Self::repair_game),
    /// this re-transfers every chunk of every file in `bundles` regardless of
    /// what is already correct on disk — including files left complete by a
    /// previous, interrupted run. To resume an interrupted install, call
    /// [`repair_game`](Self::repair_game) instead; it only re-downloads
    /// chunks that fail verification.
    ///
    /// Internally runs, in order: a file-size verification pass, disk-space
    /// and file-allocation, then the chunk downloads — reported on `tx` as
    /// [`DownloadStageEvent::FileSizeVerificationStage`],
    /// [`FileAllocationStage`](DownloadStageEvent::FileAllocationStage) and
    /// [`DownloadStage`](DownloadStageEvent::DownloadStage) respectively.
    /// Each chunk transport failure retries up to 6 times and each MD5
    /// mismatch up to 3 times, both with jittered exponential backoff; see
    /// [`crate::DownloadEvent`] for what the download stage reports as it goes.
    ///
    /// Resolves only once the whole transfer finishes or fails — drain `tx`'s
    /// paired receiver concurrently on another task.
    ///
    /// # Errors
    /// [`GogDlError::DownloadError`], notably
    /// [`crate::DownloadError::NotEnoughFreeSpace`],
    /// [`crate::DownloadError::FileAllocationError`], or a
    /// [`crate::DownloadError::ChunkHashMismatch`]/transport error once retries are
    /// exhausted for some chunk.
    pub async fn download_game(
        &self,
        bundles: Vec<ProductBundle>,
        path: &str,
        tx: mpsc::UnboundedSender<DownloadStageEvent>,
    ) -> Result<(), GogDlError> {
        self.downloader.download_game(bundles, path, tx).await?;
        Ok(())
    }
    /// Verifies `bundles` against what's on disk under `path`, then
    /// downloads only the chunks that are missing or fail their checksum.
    ///
    /// **This is the resume path.** Since this crate has no separate
    /// pause/resume mechanism, calling this on a partially-downloaded
    /// install (e.g. after [`download_game`](Self::download_game) was
    /// interrupted) is how you continue it without re-transferring
    /// already-correct data.
    ///
    /// Reports the same stages as [`download_game`](Self::download_game),
    /// plus an extra [`DownloadStageEvent::VerificationStage`] pass (one
    /// [`VerificationEvent`] per chunk) between file allocation and the
    /// download stage. Same retry policy as `download_game`.
    ///
    /// Resolves only once the whole operation finishes or fails — drain
    /// `tx`'s paired receiver concurrently on another task.
    ///
    /// # Errors
    /// Same [`GogDlError::DownloadError`] variants as
    /// [`download_game`](Self::download_game).
    pub async fn repair_game(
        &self,
        bundles: Vec<ProductBundle>,
        path: &str,
        tx: mpsc::UnboundedSender<DownloadStageEvent>,
    ) -> Result<(), GogDlError> {
        self.downloader.repair_game(bundles, path, tx).await?;
        Ok(())
    }
    /// Registers a callback invoked whenever an internal token refresh
    /// rotates the auth tokens (e.g. mid-download, on a 401). This is the
    /// only signal that the refresh token changed — persist the new
    /// [`crate::Auth`] from it, or a later [`restore_auth`](Self::restore_auth)
    /// will restore a dead refresh token and fail. Replaces any
    /// previously-registered observer.
    pub async fn set_token_observer(&self, observer: Arc<dyn TokenObserver>) {
        self.client.set_token_observer(observer).await;
    }
    /// Unregisters the observer set by
    /// [`set_token_observer`](Self::set_token_observer), if any.
    pub async fn remove_token_observer(&self) {
        self.client.remove_token_observer().await;
    }

    /// Obtains a game-scoped auth grant for GOG's cloud saves service.
    ///
    /// Cloud saves are authenticated per game: the user's current refresh
    /// token is exchanged at `auth.gog.com/token` using the `client_id` and
    /// `client_secret` from the build metadata of `build_name`, yielding
    /// tokens tied to that game's client rather than to this crate's own
    /// session. The session's own [`crate::Auth`] is left untouched, and no
    /// [`TokenObserver`] is notified.
    ///
    /// `build_name` is matched exactly against
    /// [`GameBuild::version_name`](crate::GameBuild::version_name) among the
    /// builds returned by [`get_game_builds`](Self::get_game_builds) (cached
    /// per `game_id`). The build metadata and the token exchange are fetched
    /// fresh on every call — nothing here is cached, so persist the returned
    /// [`SavesAuth`] yourself if you need it again before it expires.
    ///
    /// **Requires a still-valid session access token**, even though only the
    /// refresh token is sent: unlike other authenticated calls, this does not
    /// refresh an expired session first, and the exchange itself is not
    /// retried.
    ///
    /// # Errors
    /// - [`SavesError::BuildNotFound`] if no build of `game_id` has a
    ///   `version_name` equal to `build_name`.
    /// - [`SavesError::GamesError`] if listing the game's builds fails.
    /// - [`SavesError::DepotError`] if fetching the build metadata fails.
    /// - [`SavesError::ClientError`] wrapping
    ///   [`crate::AuthError::NotAuthenticated`] if no session is logged in,
    ///   [`crate::AuthError::TokenExpired`] if the session's access token has
    ///   expired, or [`crate::ClientError::HttpError`] if GOG rejects the
    ///   token exchange (e.g. a revoked refresh token).
    pub async fn get_saves_auth(
        &self,
        game_id: i32,
        build_name: &str,
    ) -> Result<SavesAuth, SavesError> {
        self.saves.get_saves_auth(game_id, build_name).await
    }
}
