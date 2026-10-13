# Changelog

One section per tag, newest first. Consumers pin this crate by git tag. A change of behavior
behind an unchanged signature is called out as **Behavior change**, and a signature change as
**Breaking**. The `v0.x` and `-restart` tags predate this file and aren't covered.

## 1.3.2

* `GameDetails.id` is the id passed to `get_game_details` (it was always `0`), on the fresh and the
  cached path. `GameBuilds.game_title` stays empty: GOG's builds listing carries no title, so
  filling it would take a second request. Its removal moves to `v1.4.0`. A fix; no signature
  changes.

## 1.3.1

* `GameScreenshots::resolve_links` picks its formatter by name (`product_card_screenshot_748_2x`)
  instead of `formatters.get(2)`. A link that doesn't list it uses its last formatter (GOG lists
  them smallest to largest) instead of being dropped; only a templated link with no formatter at
  all is dropped. The resolved URLs are unchanged for every product sampled, whose links all list
  the same four formatters. A fix; no signature changes.

## 1.3.0

Library and account: what the bridge's `v1.4.0` asks of this crate. **Breaking** (see below), so
consumers adopt it together with the change that fixes their `match`es and struct literals.

* `GogDl::clear_cache()` drops what can go stale so the next call fetches it again: the owned
  products, game builds, downloadable products, depot manifests (a large game's are tens of MB) and
  cloud saves remote configs. It keeps details, links, summary, screenshots, product details, build
  metadata, each product's `gamesdb` type, secure links and save ids, and leaves auth, the token
  observer and running jobs alone. Additive.
* `GogDl::remove_products(game_id, path, build_name, keep, remove)` deletes the files only
  `remove`'s depots list, never one `keep`'s also list (compared case-insensitively), prunes the
  directories it emptied and returns a `ProductRemoval { files_removed, bytes_freed }`. An empty
  `remove` fetches nothing, a missing file counts as removed, a path that would escape `path` is
  never deleted, and a dropped call is finished by calling it again. A failed delete is the new
  `FileSystemError::FileRemovalError`. Additive.
* `RemoteConfig` and its parts are `Clone`.
* **Behavior change:** `download_game` skips chunks that already verify, so it resumes like
  `repair_game` (the two now run the same pipeline; both are kept). Its events gain a
  `VerificationStage` between allocation and the download stage, `DownloadEvent::Started`'s
  `compressed_total` covers only the missing units, and a complete install emits no `DownloadStage`
  and makes no chunk request. A chunk whose real content is all zeros in a newly allocated range is
  re-fetched, as `repair_game` already did.
* **Breaking:** `OwnedGames` gains `unresolved: Vec<ProductId>`, the owned products whose `gamesdb`
  lookup failed (a transport error, any non-2xx including a 404). `get_owned_games` used to drop
  them silently and return a shorter library; it still returns `Ok`, but now `unresolved` says what
  is missing. The field is `#[serde(default)]`, so JSON saved without it still parses, and a struct
  literal must add it. A failure isn't cached, so the next call retries those products.
* **Behavior change:** one "not a game" answer. `get_game_builds`, `get_game_links` and
  `get_game_summary` return `GamesError::ProductNotAGame` for a DLC or a pack (details: `[]`;
  links: `_embedded.productType` not `GAME`; summary: `gamesdb` `type` not `game`; builds: an empty
  list whose `gamesdb` type isn't `game`, one cached extra request). `get_game_details` returned a
  decode error for a non-game, which its `ProductNotAGame` never covered; it now returns it. The
  answer is cached like a game's. The saves and download methods that list builds return
  `GamesError(ProductNotAGame)` for a non-game, not `BuildNotFound`. `gamesdb`'s `"game"` is compared
  in one place.
* `get_owned_games` no longer returns a shorter library on a failed lookup (see the `unresolved`
  entry above); each product's `gamesdb` type is cached after a successful lookup.
* Internal: `BuildMetadata` (by build link), `DepotInfo` (by manifest hash) and `RemoteConfig` (by
  game client) are cached, so `get_product_bundles`, `verify_files`, `download_game`,
  `repair_game` and the saves methods stop re-fetching them; `clear_cache` drops the last two.
  Concurrent `get_secure_links` calls for a game share one fetch, and `SecureLinksManager`,
  `SavesManager` and `DownloadManager` no longer hold their mutex across a network call.

## 1.2.3

* `DownloadEvent::Started { compressed_total: u64 }` is sent once per download stage, after
  `Prepared` and before the first `Downloading`. It is the sum of the chunks' compressed sizes over
  the units being transferred (only the missing ones for `repair_game`), so a percentage can reach
  100%: on success the `Progress` deltas minus the `ProgressRegression` deltas add up to it. Additive
  (`DownloadEvent` is `#[non_exhaustive]`), and the existing events keep their order.
* `DownloadEvent::Preparing` and `Prepared` are `#[deprecated]`. They are still sent, back-to-back,
  and are removed in `v1.4.0`. A consumer that matches them and builds with `-D warnings` gets a
  deprecation lint until it moves to `Started` or adds `#[allow(deprecated)]`.

## 1.2.2

* `download_game` and `repair_game` run one shared pipeline (size verification, allocation, and for
  `repair_game` checksum verification, then the transfer) instead of two copies of it. An internal
  refactor: no signature, event stream or behavior changes.

## 1.2.1

* **Behavior change:** `repair_game` no longer checksums chunks that file allocation just created
  as zeros: a file that was missing, or the part of a resized file past its old length. It goes
  straight to the download list, so resuming an interrupted install reads far less disk. The
  `VerificationStage` stream is unchanged in shape: each such chunk still gets one
  `VerificationEvent::ChecksumMismatch`. The one visible difference is a chunk whose real content is
  all zeros: it used to be reported `Verified` over the zero-filled file, and is now re-fetched.

## 1.2.0

Download management: what the bridge's `v1.3.0` and Lumen's `v1.3.0` ask of this crate. **Breaking**
(see below), so consumers adopt it together with the change that fixes their `match`es.

* **Breaking:** the free-space errors carry detail. `DownloadError::NotEnoughFreeSpace` and
  `ProtonError::NotEnoughFreeSpace` are `{ required: u64, available: u64 }`, and both
  `CouldNotResolveFreeSpace` variants are `{ path: PathBuf }` (they were unit variants).
* **Breaking:** the eight event enums (`DownloadEvent`, `DownloadStageEvent`, `VerificationEvent`,
  `FileAllocationEvent`, `FileSizeVerificationEvent`, `ProtonDownloadEvent`, `SavesDownloadEvent`,
  `SavesUploadEvent`), `FileSystemError`, `DownloadError`, `ProtonError` and `SavesError` are
  `#[non_exhaustive]`. A `match` on them outside the crate needs a `_ =>` arm. This is done once, so
  later patches can add variants (`v1.2.3`'s `DownloadEvent::Started`) without breaking anyone.
* **Behavior change:** a download of an unowned product fails at once. `IncorrectGameId` and
  `ProductNotOwned` from the secure-link lookup are returned without invalidating or backing off,
  where each chunk used to retry six times (~8s).
* **Behavior change:** `download_save_files` no longer truncates a good local save first. It writes a
  sibling `.<name>.gogdl-part` file, sets the modification time, syncs and renames it over the
  destination, so that file can be seen briefly in the save directory. The write completes even if
  the future is dropped: the old file or the complete new one, never a partial one. A leftover
  `.gogdl-part` file is never uploaded.
* **Behavior change:** `download_proton_release` extracts into a hidden
  `.gogdl-staging-<tag>-<random>` directory under `path`, then replaces `path/<tag>` wholesale. A
  re-download no longer overlays files the new release dropped, and a failure or a dropped future
  leaves `path` as it was. Staging directories left by a crashed run are removed at the start of the
  next download into the same `path`. A tag that sanitizes to nothing now fails before the transfer.
  The free-space check stays on the compressed size (the asset has no extracted size).
* `GogDl::get_product_sizes(game_id, build_name)` returns a `ProductSize { product_id, size,
  compressed_size }` per product of a build, summed from the build metadata's depots (after the
  download's language filter), with no manifest fetches. It equals what `get_product_bundles` sums.
* `FileSystemError`, `DepotFile` and `Chunk` are exported. `DownloadUnit` stays internal.
* The cancel-safe contract is documented: `download_game`, `repair_game`, `verify_files`,
  `download_proton_release`, `download_save_files` and `upload_save_files` each have a
  `# Cancelling` section. Dropping the future stops the work, and `repair_game` completes an install
  whose download was dropped mid-batch (tested).
* Internal: `MAX_ATTEMPTS`, `MAX_HASH_ATTEMPTS`, `backoff` and the retry helpers live in one module,
  and the unreachable `ClientError::AuthError` arm in `download_files` is gone.

## 1.1.6

Additive, plus a fix that changes what the free-space pre-flight check reads.

* `GogDl::get_free_space(path)` returns the available bytes on the disk that would hold an install
  at `path`: the same figure the download, repair and Proton pre-flight checks compare against.
  `path` needn't exist and is never created.
* **Behavior change:** the free-space check picks the disk with the **longest** mount point that
  prefixes the path, not the first. Before, `/` (listed first) won for an install on another mount,
  so the check read the root filesystem's free space. It now reads the right disk, and can fail
  where it used to pass.
* **Behavior change** (`Display` text only): `ClientError::MaxRetriesReached` now reads "Max retries
  reached" (was "Max retires reached"). It only matters to a consumer that compares strings.

## 1.1.5

No API or behavior change. Docs only: rustdoc that no longer matched the code, and a `CLAUDE.md`.

* The docs no longer say the `reqwest::Client` must set a `User-Agent` for the Proton-GE fetches.
  Both send their own (`7a57534`), so a consumer's client needs no `User-Agent` configured.
* The crate overview counts three methods that don't talk to GOG, lists the two saves transfers
  under long-running operations, and says the four cloud saves methods return `SavesError`, not
  `GogDlError`.
* `SavesError::CloudStorageNotSupported`, `RemoteConfig::get_locations`, `AuthError::TokenExpired`
  and the saves internals' descriptions match what the code does.
* `CLAUDE.md` lists the capabilities and their `GogDl` entry points, the gates and the release
  steps.

## 1.1.4

No API or behavior change. CI only: a manual `downstream` job builds `lumen-cli` and the bridge's
`rust/` crate against this commit (a `[patch]` override), so a breaking change shows up before the
tag. The CI image gained `openssh-client`.

## 1.1.3

No API change. Fix: an unauthenticated 401 no longer loops.

* **Behavior change:** `HttpClient::fetch` with `require_auth == false` (the two Proton-GE GitHub
  fetches) now returns `ClientError::HttpError` with the status and body after one request. Before,
  it re-sent the request six times back to back and returned `ClientError::MaxRetriesReached`,
  losing the 401. Transport errors are still retried with backoff, and other statuses still return
  at once.
* `GogDl::get_proton_releases`' rustdoc no longer says a 401 is retried.

## 1.1.2

No API change. Fix: the expiry margin had the wrong sign.

* **Behavior change:** `Auth::is_valid` and `SavesAuth::is_valid` now report a token as expired 60
  seconds *before* `valid_until`, as their docs always said. Before, they kept reporting it valid
  until 60 seconds *after*. `GogDl` therefore refreshes tokens up to a minute earlier, and a saves
  download or upload batch no longer starts on an already-expired grant.
* `CdnUrlParams::is_valid` already had the right direction; all three now share one implementation.

## 1.1.1

No API change. Security fix: refresh tokens no longer end up in error strings.

* The login, refresh and cloud-saves token exchanges are now POSTs with a form body, not GETs with
  the credentials in the query string, and the URL is stripped from their transport errors. Before,
  a network failure in one of them put the session's refresh token (and, for the saves exchange, a
  game's `client_secret`) in the `Display`/`Debug` of `AuthError::ClientError`,
  `SavesError::ClientError` and the `GogDlError`s wrapping them.
* **Consumers that logged those errors may have refresh tokens in old logs.** Treat them as
  exposed: log in again to get a new session, and purge or rotate the logs.

## 1.1.0

No API or behavior change: consumers can move their pin to `v1.1.0` without touching their code.
This release is the foundation for the `v1.x` line: tests, CI/CD and a pinned toolchain.

* Tests, all offline: `download_files` against a local HTTP server (retries, wrong MD5, terminal
  failure mid-batch, `ProductNotOwned`), `backoff`, `is_valid` boundaries and the `Auth`
  round-trip, and deserialization against captured GOG and GitHub responses. Integration tests in
  `tests/public_types.rs` and the crate's first doctests build the public types from outside the
  crate; each one's rustdoc has a `# Constructing in tests` section.
* GitLab CI (`lint`, `test`, `doc`) on tags and manual runs, and a GitLab release created from
  this file on every `vX.Y.Z` tag. The release job fails if the tag, `Cargo.toml` and
  `Cargo.lock` disagree.
* Every rustc and clippy warning is fixed (private renames and removals only). `rust-toolchain.toml`
  pins Rust `1.98.1` for working on this crate; a consumer builds it with its own toolchain
  (edition 2024, so Rust 1.85 or newer).
* Doc fix: `get_product_type` documents GOG's real upper-case `productType` values (`GAME`, `DLC`).

## 1.0.11

* **Behavior change:** `GogDl::get_owned_games` returns games only (ids whose `gamesdb` product
  type is `game`); DLCs and packs are no longer in `OwnedGames`. The signature is unchanged.
* Internally the owned library is now `OwnedProducts` (`get_owned_products`), and game ids are
  `ProductId`s.

## 1.0.10

* A `download_files` batch fails as soon as one unit fails terminally, without waiting for the
  rest, and the progress of cancelled or failed attempts is taken back (it nets to zero).
* **Behavior change:** `download_proton_release` renames the extracted directory to the release's
  tag (sanitized for the filesystem) and returns that path.
* Save location APIs and the path sanitizers are exposed.

## 1.0.9

* **Breaking:** `download_save_files` and `upload_save_files` take a Wine `prefix` and an
  `install_path` instead of a single `path`, and resolve cloud save locations inside the prefix.

## 1.0.8

* Save paths resolve against the game's declared cloud locations, distinct directories are kept
  when there is no remote config, and upload names match GOG's location-based paths.

## 1.0.7

* Added `GogDl::download_save_files` and `upload_save_files`, with `SavesDownloadEvent` and
  `SavesUploadEvent`.

## 1.0.6

* The remote config request uses the game's client ID.
* **Breaking:** `RemoteConfig::get_locations` is no longer `async`.

## 1.0.5

* Added `GogDl::get_remote_config`, `RemoteConfig` and `CloudStorageLocation`.

## 1.0.4

* **Breaking:** `get_save_files` returns `Vec<SaveFile>` (`SaveFiles` is gone).
* **Breaking:** removed `GogDl::get_saves_auth` and the `SavesAuth` re-export; saves tokens are
  handled internally, per game.

## 1.0.3

* Added `GogDl::get_save_files` and `get_game_save_ids`. Saves tokens and game credentials are
  cached.

## 1.0.2

* Added cloud saves authentication: `GogDl::get_saves_auth`, `SavesAuth` and `SavesError`.

## 1.0.1

* Proton-GE asset selection takes the `.tar.gz` that isn't `aarch64`, instead of requiring
  `x86_64` in its name.
* Tagged with `version = "1.0.0"` still in `Cargo.toml`; the tag is right and the manifest was
  not bumped.

## 1.0.0

* First `1.x` release: the `-restart` line renumbered, after removing the unused languages module.
