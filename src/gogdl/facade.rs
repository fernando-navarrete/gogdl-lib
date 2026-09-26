use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::mpsc;

use crate::client::{HttpClient, TokenObserver};
use crate::depot::{DepotManager, ProductDetails};
use crate::downloader::DownloadError;
use crate::downloader::{
    DownloadManager, DownloadableProduct, ProductBundle, ProductSize, VerificationEvent,
};
use crate::fs::free_space_at;
use crate::games::{
    GameBuilds, GameDetails, GameLinks, GameScreenshots, GameSummary, GamesManager, OwnedGames,
};
use crate::gogdl::error::GogDlError;
use crate::proton::{ProtonDownloadEvent, ProtonGeRelease, ProtonGeReleasesPage, ProtonManager};
use crate::saves::{SavesError, SavesManager};
use crate::secure_links::SecureLinksManager;
use crate::{DownloadStageEvent, RemoteConfig, SaveFile, SavesDownloadEvent, SavesUploadEvent};

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
    /// The request carries its own `User-Agent` header, which GitHub
    /// requires, so the `reqwest::Client` passed to
    /// [`new_from_client`](Self::new_from_client) needs no configuration for
    /// this call. The unauthenticated rate limit is 60 requests/hour per IP,
    /// shared with everything else on that address.
    ///
    /// # Errors
    /// [`GogDlError::ProtonError`] wrapping
    /// [`crate::ProtonError::ClientError`], notably a
    /// [`crate::ClientError::HttpError`] with `status: 403` for an exhausted
    /// rate limit — returned immediately, since the underlying fetch only
    /// retries transport errors. Any other non-success status, a 401
    /// included, is returned after one request.
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
    /// Sends its own `User-Agent` header, like
    /// [`get_proton_releases`](Self::get_proton_releases), so the client needs
    /// no configuration. Subject to the same unauthenticated rate limit (60
    /// requests/hour per IP).
    ///
    /// # Errors
    /// [`GogDlError::ProtonError`] wrapping
    /// [`crate::ProtonError::ClientError`], notably a
    /// [`crate::ClientError::HttpError`] with `status: 404` if `tag` doesn't
    /// match any release, or `status: 403` for an exhausted rate limit.
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
    /// tarball's size. `path` is the *parent* directory the release is
    /// installed into and is created (and canonicalized) if missing.
    ///
    /// The archive is extracted into a hidden staging directory,
    /// `path/.gogdl-staging-<tag>-<random>`, and its top-level directory
    /// then replaces `path/<tag_name>` (sanitized for the filesystem)
    /// regardless of what it was named inside the tarball — some Proton-GE
    /// releases ship theirs with an architecture suffix, e.g.
    /// `GE-Proton10-4-x86_64` for tag `GE-Proton10-4`, which breaks
    /// frontends expecting the directory to match the release tag. The
    /// returned [`PathBuf`] is `path/<tag_name>`; an existing directory with
    /// that name is replaced wholesale, so nothing of the old tree survives.
    ///
    /// The transfer is gated on the destination disk having at least the
    /// tarball's compressed size free before a single byte is read — a
    /// lower bound only, since the asset doesn't report its extracted size
    /// and the extracted tree is considerably larger. A re-download keeps the
    /// old tree on disk until the swap.
    /// Reports progress on `tx` as [`ProtonDownloadEvent`]: one `Downloading`
    /// with the compressed total size, then a `Progress` delta per network
    /// read, interleaved with an `Extracted` event per file/directory/symlink
    /// written to disk. `Extracted` paths reflect the archive's own layout,
    /// i.e. *before* the swap above. Resolves only once the
    /// whole operation finishes or fails — drain `tx`'s paired receiver
    /// concurrently on another task.
    ///
    /// **Not resumable** — there is no retry loop here, unlike
    /// [`download_game`](Self::download_game). On failure `path` is left as
    /// it was: the staging directory is removed and an existing
    /// `path/<tag_name>` is untouched. Staging directories of the same tag
    /// left by a crashed process are removed at the start of the next call,
    /// so don't run two downloads of the same tag into the same `path` at
    /// once.
    ///
    /// # Cancelling
    /// Dropping the future stops the transfer and closes the pipe into the
    /// extractor. Extraction runs on a blocking thread that the drop cannot
    /// interrupt: it hits the end of the stream, fails on the entry it was
    /// reading, removes the staging directory itself and exits, shortly
    /// after the drop. `path` is left as it was. Once the swap onto
    /// `path/<tag_name>` has started it completes, so the result is the old
    /// install or the new one, never a mix.
    ///
    /// # Errors
    /// [`GogDlError::ProtonError`], notably
    /// [`crate::ProtonError::NoSuitableAsset`] if `release` has no Linux
    /// x86_64 tarball, [`crate::ProtonError::NotEnoughFreeSpace`] or
    /// [`crate::ProtonError::CouldNotResolveFreeSpace`] if the pre-flight
    /// space check fails, [`crate::ProtonError::ExtractionError`] if the
    /// archive has no single top-level directory to rename or
    /// `release.tag_name` sanitizes to an empty name, or
    /// [`crate::ProtonError::Io`] for a network, disk, or extraction
    /// failure.
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
    /// The IDs of every game the authenticated account owns. DLCs, packs
    /// and other non-game products are filtered out by looking each owned
    /// product up on `gamesdb.gog.com` and keeping those whose `type` is
    /// `"game"`.
    ///
    /// Only the underlying owned-products list is cached (for the lifetime of
    /// this `GogDl`, after the first successful call). The per-product
    /// `gamesdb` lookups are **not** cached: every call issues one request per
    /// owned product, two at a time, so on a large library this is slow —
    /// cache the result yourself rather than calling it repeatedly. The
    /// returned IDs are in no particular order.
    ///
    /// A product whose `gamesdb` lookup fails (transport error after retries,
    /// a non-2xx status such as a 404 for a product `gamesdb` doesn't know,
    /// or an unexpected response shape) is silently left out rather than
    /// failing the call, so a transient failure can make an owned game
    /// missing from the result.
    ///
    /// # Errors
    /// [`GogDlError::GameError`] wrapping [`crate::GamesError::ClientError`] on
    /// auth or transport failure fetching the owned-products list. Per-product
    /// lookup failures are not reported; see above.
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
    /// The size of every product (the base game and its DLCs) in `build_name` of `game_id`, from
    /// one build-metadata request: no depot manifest is fetched, so it is far cheaper than
    /// [`get_product_bundles`](Self::get_product_bundles). The result is ordered by
    /// `product_id`.
    ///
    /// Each [`ProductSize`] sums the product's depots after the same language filter the download
    /// uses (`en-US`, `en` and language-neutral depots), so its `size` is the on-disk size that
    /// the free-space pre-flight check compares against, and its `compressed_size` is what
    /// crosses the wire. They equal the sums over the depot manifests that
    /// [`get_product_bundles`](Self::get_product_bundles) fetches (files' sizes and chunks'
    /// compressed sizes). Sum the entries of the products you selected for an install total.
    ///
    /// Every product in the build is listed, owned or not; intersect the result with
    /// [`get_downloadable_products`](Self::get_downloadable_products) to keep the owned ones.
    /// **Not cached**: every call is a fresh round-trip for the build metadata.
    ///
    /// # Errors
    /// [`GogDlError::DownloadError`] wrapping [`crate::DownloadError::BuildNotFound`] if no build
    /// named `build_name` exists.
    pub async fn get_product_sizes(
        &self,
        game_id: i32,
        build_name: &str,
    ) -> Result<Vec<ProductSize>, GogDlError> {
        let sizes = self
            .downloader
            .get_product_sizes(game_id, build_name)
            .await?;
        Ok(sizes)
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
    /// # Cancelling
    /// Dropping the future stops the verification and leaves the disk
    /// untouched, since nothing is written. Chunk checksums already running on
    /// tokio's blocking pool (one per worker at most) finish reading their
    /// chunk and their results are discarded.
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
    /// The stage opens with [`DownloadEvent::Started`](crate::DownloadEvent::Started),
    /// whose `compressed_total` is the denominator for the `Progress` deltas.
    /// A secure-link failure that a retry can't change (an unowned product or
    /// an invalid product id) is returned at once, without retrying.
    ///
    /// **The first chunk that fails terminally (retries exhausted) aborts
    /// the whole download stage.** Units already in flight are cancelled —
    /// each cancelled unit's reported bytes are taken back via
    /// [`DownloadEvent::ProgressRegression`](crate::DownloadEvent::ProgressRegression)
    /// — and units not yet started are never attempted. The error this
    /// method returns is whichever unit failed first in *time*, not
    /// necessarily first in `bundles`' order, so a failed call leaves an
    /// install in an indeterminate partial state; call
    /// [`repair_game`](Self::repair_game) afterwards to resume.
    ///
    /// Resolves only once the whole transfer finishes or fails — drain `tx`'s
    /// paired receiver concurrently on another task.
    ///
    /// # Cancelling
    /// Dropping the future is how to cancel, and it is safe: every transfer
    /// runs inside the future (no task is spawned), so the drop stops all of
    /// them, no new chunk starts, and each in-flight chunk's reported bytes
    /// are taken back with
    /// [`DownloadEvent::ProgressRegression`](crate::DownloadEvent::ProgressRegression).
    /// Nothing is cleaned up: files stay allocated at full size, chunks
    /// already written stay, and a chunk that was mid-transfer may be
    /// partially written. Call [`repair_game`](Self::repair_game) to continue;
    /// it checksums every chunk and fetches only the ones that are wrong. A
    /// disk write already handed to tokio's blocking pool may complete just
    /// after the drop, and nothing else outlives it.
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
    /// Available bytes on the disk that would hold an install at `path`: the
    /// figure [`download_game`](Self::download_game),
    /// [`repair_game`](Self::repair_game) and
    /// [`download_proton_release`](Self::download_proton_release) compare
    /// their required size against before writing anything, so a caller's own
    /// "does it fit" check agrees with theirs.
    ///
    /// The disk is the one with the longest mount point that prefixes `path`.
    /// `path` needn't exist, and is never created: its nearest existing
    /// ancestor stands in for it, since an install directory usually doesn't
    /// exist yet. Local only: no network, no auth, not cached.
    ///
    /// # Errors
    /// [`GogDlError::DownloadError`] wrapping
    /// [`crate::DownloadError::FileSystemError`], when no ancestor of `path`
    /// can be resolved (e.g. permission denied) or no disk matches it.
    pub async fn get_free_space(&self, path: &Path) -> Result<u64, GogDlError> {
        free_space_at(path)
            .await
            .map_err(|e| GogDlError::DownloadError(DownloadError::FileSystemError(e)))
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
    /// download stage. Chunks that file allocation just created as zeros are
    /// reported without being read; see
    /// [`DownloadStageEvent::VerificationStage`]. Same retry policy as
    /// `download_game`.
    ///
    /// Resolves only once the whole operation finishes or fails — drain
    /// `tx`'s paired receiver concurrently on another task.
    ///
    /// # Cancelling
    /// Same as [`download_game`](Self::download_game): drop the future to
    /// cancel it, and call this method again to continue. During the
    /// verification pass a drop also discards the results of the chunk
    /// checksums already running on tokio's blocking pool (one per worker at
    /// most), which finish reading their chunk first.
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
    /// Fetches the current user's cloud save file listing for a game from
    /// `cloudstorage.gog.com`.
    ///
    /// Cloud saves are authenticated per game: the user's current refresh
    /// token is exchanged at `auth.gog.com/token` using the `client_id` and
    /// `client_secret` from the build metadata of `build_name`, yielding a
    /// game-scoped access token that is used as the bearer token for the
    /// listing. The session's own [`crate::Auth`] is left untouched, and no
    /// [`TokenObserver`] is notified. The game's storage is addressed by that
    /// same `client_id`.
    ///
    /// `build_name` is matched exactly against
    /// [`GameBuild::version_name`](crate::GameBuild::version_name) among the
    /// builds returned by [`get_game_builds`](Self::get_game_builds) (cached
    /// per `game_id`). The game's client credentials and the game-scoped
    /// grant are cached in memory per `(game_id, build_name)` for the
    /// lifetime of this `GogDl`; the grant is reused until it is about to
    /// expire and then exchanged again. Neither cache is persisted, and the
    /// listing itself is never cached.
    ///
    /// Returns one [`SaveFile`] per stored file, in the order GOG lists them;
    /// an empty `Vec` if the game has no cloud saves.
    ///
    /// **Requires a still-valid session access token**, even though only the
    /// refresh token is sent: unlike other authenticated calls, this does not
    /// refresh an expired session first. Neither the token exchange nor the
    /// storage request is retried, including on network errors.
    ///
    /// # Errors
    /// - [`SavesError::BuildNotFound`] if no build of `game_id` has a
    ///   `version_name` equal to `build_name`.
    /// - [`SavesError::GamesError`] if listing the game's builds fails.
    /// - [`SavesError::DepotError`] if fetching the build metadata fails.
    /// - [`SavesError::ClientError`] wrapping
    ///   [`crate::AuthError::NotAuthenticated`] if no session is logged in,
    ///   or [`crate::AuthError::TokenExpired`] if the session's access token
    ///   has expired.
    /// - [`SavesError::ClientError`] wrapping
    ///   [`crate::ClientError::HttpError`] if GOG rejects the token exchange
    ///   (e.g. a revoked refresh token) or the storage request returns any
    ///   non-success status, including 401;
    ///   [`crate::ClientError::NetworkError`] if either request fails in
    ///   transit; or [`crate::ClientError::DeserializationError`] if a
    ///   response doesn't match the expected shape.
    pub async fn get_save_files(
        &self,
        game_id: i32,
        build_name: &str,
    ) -> Result<Vec<SaveFile>, SavesError> {
        self.saves.get_save_files(game_id, build_name).await
    }
    /// Fetches a game's Galaxy client remote configuration from
    /// `remote-config.gog.com`.
    ///
    /// The returned [`RemoteConfig`] is the metadata needed to map the
    /// listing from [`get_save_files`](Self::get_save_files) onto a local
    /// install: whether the game supports cloud saves at all
    /// ([`is_supported`](RemoteConfig::is_supported)), and which directories
    /// it reads and writes them in
    /// ([`get_locations`](RemoteConfig::get_locations)). Both answer for the
    /// game's Windows client only. The document is requested for Galaxy
    /// client component version `2.0.43`.
    ///
    /// `build_name` is matched exactly against
    /// [`GameBuild::version_name`](crate::GameBuild::version_name) among the
    /// builds returned by [`get_game_builds`](Self::get_game_builds) (cached
    /// per `game_id`), and serves only to resolve the game's own client
    /// credentials, which are cached in memory per `(game_id, build_name)`
    /// for the lifetime of this `GogDl` and shared with
    /// [`get_save_files`](Self::get_save_files). The remote config request
    /// itself is unauthenticated and is neither cached nor retried,
    /// including on network errors.
    ///
    /// **Requires a logged-in session** even so, because resolving those
    /// credentials lists the game's builds and fetches build metadata; both
    /// are authenticated calls, and unlike
    /// [`get_save_files`](Self::get_save_files) they do refresh an expired
    /// session access token first.
    ///
    /// # Errors
    /// - [`SavesError::BuildNotFound`] if no build of `game_id` has a
    ///   `version_name` equal to `build_name`.
    /// - [`SavesError::GamesError`] if listing the game's builds fails.
    /// - [`SavesError::DepotError`] if fetching the build metadata fails.
    /// - [`SavesError::ClientError`] wrapping
    ///   [`crate::ClientError::HttpError`] if GOG publishes no remote config
    ///   for the game or otherwise rejects the request,
    ///   [`crate::ClientError::NetworkError`] if it fails in transit, or
    ///   [`crate::ClientError::DeserializationError`] if the document
    ///   doesn't match the expected shape.
    ///
    /// [`SavesError::CloudStorageNotSupported`] is never returned here — it
    /// comes from [`RemoteConfig::get_locations`] on the returned value.
    pub async fn get_remote_config(
        &self,
        game_id: i32,
        build_name: &str,
    ) -> Result<RemoteConfig, SavesError> {
        self.saves.get_remote_config(game_id, build_name).await
    }
    /// Downloads a game's cloud saves into the Wine prefix `prefix`,
    /// reporting progress one file at a time through `tx`.
    ///
    /// `prefix` is the root of the Wine (or Proton `compatdata`) prefix the
    /// game runs in, the directory holding `drive_c`. Each of the game's
    /// [`CloudStorageLocation`](crate::CloudStorageLocation)s from
    /// [`get_remote_config`](Self::get_remote_config) is expanded to a
    /// directory: GOG writes a location as `<?VARIABLE?>/path`, and the
    /// variable stands for a directory of the Windows profile, which lives at
    /// `<prefix>/drive_c/users/<user>`:
    ///
    /// | Variable                     | Directory                 |
    /// |------------------------------|---------------------------|
    /// | `SAVED_GAMES`                | `<user>/Saved Games`      |
    /// | `DOCUMENTS`                  | `<user>/Documents`        |
    /// | `APPLICATION_DATA_ROAMING`   | `<user>/AppData/Roaming`  |
    /// | `APPLICATION_DATA_LOCAL`     | `<user>/AppData/Local`    |
    /// | `APPLICATION_DATA_LOCAL_LOW` | `<user>/AppData/LocalLow` |
    /// | `INSTALL`                    | `install_path`            |
    ///
    /// `install_path` is the game's install directory. Only `<?INSTALL?>`
    /// locations use it, so it may be any path for a game that has none.
    ///
    /// `<user>` is found by looking in `<prefix>/drive_c/users`; the first
    /// that exists wins: `steamuser` (Proton), then the host's `$USER` or
    /// `$USERNAME` (plain Wine), then the only directory there that is not
    /// `Public`. It is only looked up when a location needs it. The text
    /// after the variable comes from GOG and is treated as untrusted:
    /// separators are normalized and `..` components are dropped, so a
    /// location cannot leave its variable's directory.
    ///
    /// Every file in the [`get_save_files`](Self::get_save_files) listing is
    /// downloaded into the directory of the location it belongs to, at
    /// [`SaveFile::relative_path_in`] beneath it — its cloud name with the
    /// location's segment removed. A game whose location is named `saves`
    /// has `saves/AutoSave-0/sav.dat` land at
    /// `<directory>/AutoSave-0/sav.dat`; one whose location is `__default`
    /// has `saves/__default/profile/slot1.sav` land at
    /// `<directory>/profile/slot1.sav`. A game with several locations gets
    /// each in its own directory. A file whose name matches none of the
    /// locations goes into the first one, with only a leading `saves/`
    /// removed. Any other directory in the name is kept, so files that differ
    /// only in their directory stay apart. A location's directory is created
    /// when the first file for it arrives; nothing is ever written outside
    /// it. Existing files are overwritten, but only once the new one is
    /// complete: see below.
    ///
    /// Each file is received in full, verified against the `ETag` GOG sends
    /// with it (an MD5 of the stored bytes; a response without one is
    /// accepted unchecked), decompressed, and given the modification time GOG
    /// recorded when the file was uploaded. It is written to a temporary
    /// `.<name>.gogdl-part` file beside its destination, flushed to disk, and
    /// renamed over the destination, so an existing save is replaced only
    /// once the new one is on disk. A failure at any point leaves it as it
    /// was.
    ///
    /// # Progress
    ///
    /// Reported as [`SavesDownloadEvent`]s: one `Preparing`, then for each
    /// file `FileStarted`, some `Progress` deltas, and `FileFinished`. Files
    /// are downloaded one at a time, so `Progress` always belongs to the most
    /// recent `FileStarted`. The future resolves only when the whole job is
    /// done; drain the receiver on another task meanwhile.
    ///
    /// **Nothing is retried**, including on network errors. The first
    /// failure aborts the call, and files downloaded before it stay on disk.
    ///
    /// # Cancelling
    /// Dropping the future stops the call between or during files. Files
    /// downloaded before the drop stay. A file still being received is
    /// discarded, and the save on disk is untouched. The write itself is one
    /// blocking task that a drop cannot interrupt: it finishes, so each save
    /// ends up either as it was or fully replaced, never truncated or partial,
    /// and no `.gogdl-part` file is left. The remaining files are not touched.
    /// Authentication, caching and the need for a still-valid session access
    /// token are as for [`get_save_files`](Self::get_save_files).
    ///
    /// # Errors
    /// - Everything [`get_remote_config`](Self::get_remote_config) and
    ///   [`get_save_files`](Self::get_save_files) can return. The remote
    ///   config is required: without the game's locations there is no
    ///   directory to download into.
    /// - [`SavesError::CloudStorageNotSupported`] if the game declares no
    ///   cloud save location.
    /// - [`SavesError::InvalidSaveLocation`] or
    ///   [`SavesError::UnknownSaveLocationVariable`] if a location is not a
    ///   `<?VARIABLE?>` this crate knows, e.g. the macOS-only
    ///   `<?APPLICATION_SUPPORT?>`.
    /// - [`SavesError::WineUserDirNotFound`] if a location needs the user
    ///   directory and none can be found in `prefix`.
    /// - [`SavesError::InvalidSaveFileName`] if a listed name has no path
    ///   left once its `saves/` prefix and location segment are removed.
    /// - [`SavesError::FileSystemError`] if a location's directory, or a
    ///   directory for one of the files, cannot be created or resolved.
    /// - [`SavesError::ClientError`] wrapping
    ///   [`crate::ClientError::HttpError`] if GOG rejects a download (e.g. an
    ///   expired token), or [`crate::ClientError::NetworkError`] if one fails
    ///   in transit.
    /// - [`SavesError::HashMismatch`] if a file's bytes do not match its
    ///   `ETag`; that file is not written.
    /// - [`SavesError::InvalidHeader`] if a file's modification time header
    ///   cannot be parsed; that file is not written.
    /// - [`SavesError::Io`] if decompressing or writing a file fails.
    pub async fn download_save_files(
        &self,
        game_id: i32,
        build_name: &str,
        prefix: &Path,
        install_path: &Path,
        tx: mpsc::UnboundedSender<SavesDownloadEvent>,
    ) -> Result<(), SavesError> {
        self.saves
            .download_save_files(game_id, build_name, prefix, install_path, tx)
            .await
    }
    /// Uploads a game's saves from the Wine prefix `prefix` to its cloud
    /// saves, reporting progress one file at a time through `tx`. The
    /// counterpart of [`download_save_files`](Self::download_save_files), and
    /// `prefix` and `install_path` mean the same thing: the game's save
    /// locations are expanded to directories in the same way, as documented
    /// there.
    ///
    /// The directory of each location is walked recursively and every
    /// regular file in it is uploaded; symlinks and the `.gogdl-part` files
    /// an interrupted download can leave behind are skipped. A location whose
    /// directory does not exist has nothing to upload and is skipped. A file
    /// at `<directory>/profile/slot1.sav` is stored as
    /// `<location>/profile/slot1.sav`, where `<location>` is the
    /// [`name`](crate::CloudStorageLocation::name) of the location the
    /// directory belongs to — for a game whose location is named `saves`,
    /// `<directory>/AutoSave-0/sav.dat` is stored as
    /// `saves/AutoSave-0/sav.dat`, replacing the object the game already has.
    /// A game with several locations has each uploaded under its own name.
    /// Files already in the cloud are overwritten; files that exist only in
    /// the cloud are left alone — nothing is ever deleted, and nothing is
    /// compared, so unchanged files are uploaded again.
    ///
    /// Each file is gzip-compressed at level 6 and sent with its local
    /// modification time and the MD5 of the compressed bytes. The whole
    /// compressed file is held in memory while it is sent.
    ///
    /// # Progress
    ///
    /// Reported as [`SavesUploadEvent`]s: one `Preparing`, then for each file
    /// `FileStarted`, some `Progress` deltas, and `FileFinished`. Files are
    /// uploaded one at a time, so `Progress` always belongs to the most
    /// recent `FileStarted`. Deltas count compressed bytes as they are
    /// queued for sending, not as GOG acknowledges them. Drain the receiver
    /// on another task while the future runs.
    ///
    /// **Nothing is retried**, including on network errors. The first
    /// failure aborts the call, and files uploaded before it stay uploaded.
    ///
    /// # Cancelling
    /// Dropping the future abandons the transfer in progress. Local files are
    /// only ever read, so they are untouched. Files uploaded before the drop
    /// stay uploaded; whether GOG stored the one in flight is up to its
    /// storage, so upload again to be sure.
    /// Authentication, caching and the need for a still-valid session access
    /// token are as for [`get_save_files`](Self::get_save_files).
    ///
    /// # Errors
    /// - Everything [`get_remote_config`](Self::get_remote_config) can
    ///   return, plus [`SavesError::CloudStorageNotSupported`] if the game
    ///   declares no cloud save location.
    /// - [`SavesError::InvalidSaveLocation`] or
    ///   [`SavesError::UnknownSaveLocationVariable`] if a location is not a
    ///   `<?VARIABLE?>` this crate knows, e.g. the macOS-only
    ///   `<?APPLICATION_SUPPORT?>`.
    /// - [`SavesError::WineUserDirNotFound`] if a location needs the user
    ///   directory and none can be found in `prefix`.
    /// - [`SavesError::Io`] if a location's directory cannot be walked or a
    ///   file cannot be read or compressed.
    /// - [`SavesError::InvalidSaveFileName`] if a file's path is not valid
    ///   UTF-8.
    /// - [`SavesError::ClientError`] wrapping
    ///   [`crate::ClientError::HttpError`] if GOG rejects an upload, or
    ///   [`crate::ClientError::NetworkError`] if one fails in transit.
    pub async fn upload_save_files(
        &self,
        game_id: i32,
        build_name: &str,
        prefix: &Path,
        install_path: &Path,
        tx: mpsc::UnboundedSender<SavesUploadEvent>,
    ) -> Result<(), SavesError> {
        self.saves
            .upload_save_files(game_id, build_name, prefix, install_path, tx)
            .await
    }
}
