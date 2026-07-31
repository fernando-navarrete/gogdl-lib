# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

`gogdl-lib` is a Rust library (crate name `gogdl-lib`, edition 2024) for talking to GOG's Galaxy backend: authentication, catalog browsing, downloading/repairing/verifying game installs, cloud saves, and Proton-GE release downloads. It has no binary target — consumers depend on it as a library. `api.md` at the repo root is the canonical, exhaustive public API reference (every type, method, and non-obvious behavior); read it before making changes to public surface, and keep it in sync with any public API change.

## Commands

```sh
cargo build              # build the library
cargo test --lib         # run the full unit test suite (~26 tests, all in-tree)
cargo test <substring>   # run a subset, e.g. `cargo test adaptive::`
cargo check               # fast type-check without codegen
cargo clippy               # lint
cargo fmt                  # format
```

Tests live inline in `mod tests` blocks within the source files they cover (not a separate `tests/` directory) — e.g. `src/downloader/adaptive.rs`, `src/downloader/util/path_resolver.rs`, `src/client/client.rs`, `src/proton/release.rs`, `src/saves/*.rs`. Some tests (adaptive controller) use `tokio::time::pause`/`advance` (the `tokio` `test-util` feature, a dev-dependency) to drive timers deterministically instead of racing real sleeps — follow that pattern for any new time-dependent test rather than using real `sleep`s.

## Architecture

**`GogDl` (`src/gogdl/gogdl.rs`) is the single public entry point.** Every operation is a thin async method on `GogDl` that delegates to one internal manager and maps errors into the crate-wide `GogDlError`. `GogDl` owns one instance of each manager, constructed once in `new_from_client` and cloned into each other as needed:

- `AuthManager` (`src/auth`) — login/token refresh/persistence; other managers hold a clone of it and call it for authenticated requests.
- `GamesManager` (`src/games`) — catalog: owned games, game details/builds/links/summary/screenshots.
- `DepotManager` (`src/depot`) — build manifests / product details / depot metadata.
- `SecureLinksManager` (`src/secure_links`) — resolves CDN download links for a product.
- `DownloadManager` (`src/downloader`) — download/repair/verify orchestration; the largest and most complex module (see below).
- `SavesManager` (`src/saves`) — cloud save list/download/upload/delete.
- `ProtonManager` (`src/proton`) — Proton-GE release listing/download.
- `HttpClient` (`src/client`) — thin wrapper around the caller-supplied `reqwest::Client`, shared (via `.clone()`) by every manager above.

Every module follows the same shape: a private `mod.rs` per submodule that only re-exports the curated public surface, plus a per-module `error.rs` (a `thiserror` enum wrapping the module's failure modes and lower-level errors like `reqwest`/`serde_json`/`std::io` via `#[from]`). `GogDlError` (`src/gogdl/error.rs`) wraps each module's error enum in one outer variant per subsystem (`AuthError`, `GamesError`, `DownloadError`, etc.) — this is the one error type every public `GogDl` method returns.

**Type reachability is deliberately curated.** Several public structs (e.g. `DownloadableFiles`) have fields typed with structs from a private submodule that is *not* re-exported at the crate root (e.g. `DepotFile`). Field access, iteration, and method calls on these fields work fine via type inference; only naming the type in a signature fails to compile. When adding new public structs with nested data, follow this pattern deliberately rather than exporting every intermediate DTO — see api.md §9 for the full list of affected types before changing it.

**Caching:** catalog/manifest lookups (`get_game_builds`, `get_downloadable_files`, `get_product_details`, etc.) are cached in-memory, unbounded, for the lifetime of the `GogDl` instance, with no TTL/invalidation — a caller wanting fresh data must construct a new `GogDl`. Some caches have deliberate quirks (`get_owned_games` only counts as cached once non-empty; `get_game_details` negative-caches decode failures as `ProductNotAGame`) — see api.md §4 "Non-obvious behavior" before touching cache logic.

### Downloader internals (`src/downloader`)

This is the most involved module:

- `downloader.rs` / `download_manager.rs` — top-level download/repair/verify orchestration.
- `adaptive.rs` — an adaptive concurrency controller that hill-climbs concurrent chunk downloads between `DownloadConfig::min_concurrency`/`max_concurrency` against measured throughput, backing off on transient error bursts. Driven by `sample_interval`/`step` in `DownloadConfig` (`config.rs`).
- `stream.rs` — chunk-level HTTP streaming/decoding (zlib via `flate2`) and retry/timeout handling.
- `stages.rs` — allocation/download/verify stage sequencing and event emission.
- `events.rs` — the public progress event types (`DownloadJobEvent`, `DownloadEvent`, `RepairEvent`, `VerifyEvent`, etc.) sent over caller-owned `mpsc::UnboundedSender` channels. Progress is push-based: the job's own `async fn` only resolves on completion/failure, all incremental state goes over the channel.
- `util/path_resolver.rs` — resolves manifest paths safely under the install root (rejects path traversal, sanitizes reserved Windows filenames); has concurrency-safety tests for concurrent resolves of the same directory.
- `util/hash.rs` — checksum verification (MD5/SHA-256, used for saves/Proton and repair/verify flows).

When changing progress-channel event shapes or downloader retry/timeout behavior, update the corresponding section of `api.md` (§5) in the same change — it's treated as the authoritative spec for consumers, not incidental documentation.
