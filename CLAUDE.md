# CLAUDE.md

Guidance for Claude Code when working in this repository.

## What this is

`gogdl-lib` is a Rust client for GOG's Galaxy backend: authentication, catalog browsing,
downloading/repairing/verifying game installs, cloud saves, and Proton-GE releases. **`GogDl`
(`src/gogdl/facade.rs`) is the only entry point.** Every module is private, and everything public
goes through the re-exports in `src/lib.rs`; every other public type is data you get back or hand in.
Rustdoc (enforced by `#![warn(missing_docs)]`) is the per-item spec. This file only tracks what
exists and how to work on it.

All GOG and GitHub hosts are hardcoded (`src/constants/mod.rs` and `format!`s in the fetchers).
There is no injectable base URL, so offline tests seed caches instead (see **Tests**).

## Consumers and pinning

- **`gogdl_flutter`** (`../gogdl_flutter`, the Flutter/Rust bridge, in its `rust/` crate), and
  **Lumen** through it.
- **`lumen-cli`** (`../lumen-cli`), which exercises this crate directly.

Both pin this crate **by git tag**, not by semver range. So a signature change, and a change of
behavior behind an unchanged signature, is allowed in a **minor** only (never a patch), and ships
with the consumer change that adopts it. `ROADMAP.md` has the versioning rules and which release
serves which consumer stage. The `rust-toolchain.toml` here applies to this repo only; cargo ignores
a dependency's.

## Capabilities and entry points

All are methods on `GogDl`. Every fallible one returns `GogDlError` **except the four cloud saves
ones, which return `SavesError`** (`GogDlError::SavesError` is planned for `v1.4.0`, GAPS "Medium —
cloud saves"). The long-running ones report progress on an `mpsc::UnboundedSender` the caller
drains concurrently.

| Area | Methods |
|---|---|
| Construction | `new_from_client` |
| Auth | `get_login_url`, `login_with_code`, `restore_auth`, `set_token_observer`, `remove_token_observer` |
| Library | `get_owned_games`, `get_game_details`, `get_game_builds`, `get_game_links`, `get_game_summary`, `get_game_screenshots`, `get_product_details` |
| Downloads, repair, verify | `get_downloadable_products`, `get_product_bundles`, `verify_files`, `download_game`, `repair_game`, `get_free_space` |
| Proton-GE (GitHub, no GOG auth) | `get_proton_releases`, `get_proton_release_by_tag`, `download_proton_release` |
| Cloud saves | `get_save_files`, `get_remote_config`, `download_save_files`, `upload_save_files` |

- **Auth** state lives in the `HttpClient`; refreshes happen inside requests and are reported to a
  registered `TokenObserver`, the only signal that the refresh token rotated. The cloud saves grant
  is a separate per-game token that is never refreshed or reported.
- **Library** results are cached per id for the lifetime of the `GogDl`, except `get_owned_games`,
  which caches only the owned-products list and does one uncached `gamesdb` lookup per product.
- **Downloads:** `download_game` re-transfers everything; `repair_game` is the resume path;
  `get_product_bundles` is not cached. Events: `DownloadStageEvent`, `DownloadEvent`,
  `VerificationEvent`.
- **Proton:** `download_proton_release` extracts the tarball as it streams, emitting
  `ProtonDownloadEvent`. Not resumable.
- **Cloud saves:** `download_save_files`/`upload_save_files` take a Wine prefix and the install path
  and expand the game's save locations into it; events are `SavesDownloadEvent` and
  `SavesUploadEvent`. Nothing is retried.

When a method is added, changed or removed on `GogDl`, update this table and `CHANGELOG.md`.

## Layout

- `src/lib.rs`: crate docs and every public re-export.
- `src/gogdl/`: the `GogDl` facade and `GogDlError`.
- `src/client/`: `HttpClient` (`http.rs`: fetch, retry, streaming, token POSTs) and `auth/` (`Auth`,
  refresh, the `Expiring` trait that gives all three `is_valid`s one margin, `TokenObserver`).
- `src/games/`, `src/depot/`: catalog and build/product metadata.
- `src/secure_links/`: CDN secure-link cache, the seam offline download tests use.
- `src/downloader/`: the engine (`engine.rs`), bundles, progress events, `util/` (backoff, MD5,
  `ProgressGuard`, offset writer).
- `src/proton/`: GitHub release fetchers and the download/extract pipeline.
- `src/saves/`: listing, remote config, save-location expansion, the downloader and uploader.
- `src/fs/`, `src/constants/`: path resolution and free space; hosts and constants.
- `src/test_support.rs` (`#[cfg(test)]`): a raw-`tokio` HTTP fixture server on `127.0.0.1` that can
  script replies per request and hold a connection open mid-body.
- `tests/public_types.rs` and `tests/fixtures/`: an integration test that builds every public type
  from outside the crate, and scrubbed captures of real responses.
- `ci/`, `tool/`, `.gitlab-ci.yml`: see **Gates** and **Releasing**.
- `ROADMAP.md`, `GAPS.md`, `CHANGELOG.md`, `devlog/` (decisions and pitfalls per finished line).

## Tests

Everything runs offline and in well under a second. Tests point the downloader at the local fixture
by seeding `SecureLinksManager`'s cache (`fixture_links`, `lookups()`), and use `tokio::time::pause`
so `backoff` doesn't take real time. `test-util` is a dev-dependency feature only. Fixtures must have
secrets and account ids scrubbed. What is still untested is listed in GAPS "Medium — missing
coverage".

## Gates

Run all four before committing; the first three are the CI jobs (`lint`, `test`, `doc`):

```bash
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked                                  # unit tests, tests/, doctests
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --locked
```

The toolchain is pinned in `rust-toolchain.toml`. CI runs in `gogdl-lib-ci:rust-<version>`
(`ci/Dockerfile`), built on the runner host with `ci/build-image.sh`; rebuild it and update `image:`
in `.gitlab-ci.yml` when the channel changes (the `.toolchain` check fails a job on a stale image).
Pipelines run only for tags and manual runs from the web UI. The manual `downstream` job builds
`lumen-cli` and the bridge's `rust/` against this checkout through a `[patch]`, and is the check for
a breaking change.

## Releasing

One tag per release, on the working branch (not `main`), as `ROADMAP.md`'s versioning rules say:

1. Bump `version` in `Cargo.toml` to match the tag, and commit `Cargo.lock` with it.
2. Add a `## X.Y.Z` section to `CHANGELOG.md`; mark a behavior change or a signature change.
3. Tick the item(s) in `GAPS.md`.
4. Commit (`vX.Y.Z: ...`), tag `vX.Y.Z`, push both.
5. The tag pipeline's `release` job runs `tool/release_notes.sh <tag>`, which fails unless the tag,
   `Cargo.toml`, `Cargo.lock` and a non-empty `CHANGELOG.md` section agree, then creates the GitLab
   release with that section. Check it is green.

`devlog/v1.1.0-foundation.md` records the decisions and pitfalls of the `v1.1.x` line.
