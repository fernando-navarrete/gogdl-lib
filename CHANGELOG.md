# Changelog

One section per tag, newest first. Consumers pin this crate by git tag. A change of behavior
behind an unchanged signature is called out as **Behavior change**, and a signature change as
**Breaking**. The `v0.x` and `-restart` tags predate this file and aren't covered.

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
