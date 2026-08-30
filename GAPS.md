# GAPS.md

Findings from a full read-through of `gogdl-lib` on the `restart` branch (all `src/*.rs`, `Cargo.toml`,
`cargo build`, `cargo clippy --all -- -W clippy::all`) on 2026-08-30. Grouped by severity, each item has
a checkbox so they can be worked one at a time. File:line references are accurate as of commit
`377314c`.

No automated tests exist yet (`grep -r '#\[test\]' src/` is empty), so treat every fix here as
untested until you add coverage for it.

---

## High — correctness bugs

- [ ] **`refresh_auth` persists tokens without `valid_until`.**
  `src/auth/auth_manager.rs:59-66` builds the JSON string returned to the caller *before* setting
  `response.valid_until`, then sets `valid_until` only on the in-memory copy stored afterwards.
  `login_with_code` (lines 33-43) does it in the opposite, correct order. Any app that persists the
  string returned by `refresh_auth()` to disk will save `"valid_until":null` every time, silently
  losing the computed expiry.

- [ ] **Silent, internal token refreshes are invisible to callers.**
  Six call sites (`depot/depot_info.rs`, `depot/build_metadata.rs`, `games/owned_games.rs`,
  `games/game_details.rs`, `games/game_build.rs`, `secure_links/secure_links.rs`) each catch a 401,
  call `auth.refresh_auth()`, and retry — but discard the returned JSON string. `GogDl::refresh_auth()`
  is the only path that surfaces a refreshed token to the app. Since GOG rotates refresh tokens, an
  app that only persists tokens when it explicitly calls `refresh_auth()` can end up with a stale
  refresh token on disk after a transparent in-request refresh, and fail to log back in on next
  launch even though the session was healthy seconds before.

- [ ] **`valid_until` is dead.** It's computed and stored on every login/refresh
  (`src/auth/model.rs:15`) but nothing ever reads it — there is no proactive "is my token about to
  expire" check anywhere. Expiry is only ever discovered reactively via a 401. Either wire it into a
  pre-flight check or remove it.

## High — panics on untrusted data

- [ ] **Two `.parse().unwrap()` calls can crash the process on bad input**, with no handling for the
  identical failure a few lines away in a sibling file:
  - `src/secure_links/links_manager.rs:49` — `game_id.parse().unwrap()`, where `game_id: &str` is a
    public API parameter (ultimately comes from the Flutter/Dart side across FFI). A malformed
    caller-supplied id panics the whole process rather than returning an error.
  - `src/downloader/downloadable_product.rs:61` — `product_id.parse().unwrap()` on a value taken from
    the GOG API response.
  - Compare with `src/downloader/product_bundle.rs:52`, which parses the *same kind* of value with
    `.parse().unwrap_or(0)` — i.e. the two near-duplicate code paths (`get_downloadable_products` vs
    `get_download_files`) handle the same possible failure inconsistently, one by panicking and one by
    silently substituting `0`.

## High — downloader reliability

- [ ] **No retry/backoff for transient network failures during download.**
  `Downloader::download_files` (`src/downloader/downloader.rs:156-224`) downloads every chunk exactly
  once; a single timeout, connection reset, or 5xx on any one chunk aborts the *entire* multi-gigabyte
  download via `results.into_iter().collect::<Result<(), DownloadError>>()?`. There is no retry
  anywhere in the crate (`grep -ri retry src/` is empty).

- [ ] **One failed file allocation aborts the whole download.**
  `Downloader::download` (`src/downloader/downloader.rs:82-85`) fails the entire job the moment
  `allocate_missing_files` reports *any* failure, instead of skipping/reporting just the offending
  file(s) and continuing with the rest.

- [ ] **`Downloader::verify` discards its own result.**
  `src/downloader/downloader.rs:118-122` computes `_missing_units` and then always returns `Ok(())`
  regardless of what verification found. A caller that isn't listening on the `tx` event channel has
  no way to learn verification failed.

- [ ] **Blocking syscalls run directly inside async tasks.**
  `PathResolver::open_file` (`src/downloader/fs/path_resolver.rs:113-123`) uses `std::fs::OpenOptions`
  synchronously instead of `tokio::fs`, unlike `allocate_file` right above it which correctly uses the
  async version. Worse, every downloaded chunk is written via `OffsetWriter::write` →
  `File::write_at` (`src/downloader/util/offset_writer.rs:41`), a blocking positioned write called
  straight from the `stream_chunk` async loop (`src/downloader/downloader.rs:193-198`) — never wrapped
  in `spawn_blocking`. With up to 12 concurrent downloads (`Downloader::new`'s thread clamp) this can
  stall tokio's worker threads under load.

- [ ] **Secure links are cached forever with no expiry handling.**
  `SecureLinksManager::links_cache` (`src/secure_links/links_manager.rs:21,42-47`) never evicts an
  entry, even though the fetched `CdnUrlParams` carries `expires_at`/`ttl` fields
  (`src/secure_links/secure_links.rs:11,13`) that are parsed but never consulted. A link that expires
  mid-download (long transfer, or a second job that reuses a stale cache entry) will start failing CDN
  requests with no refresh path — the crate only ever retries on 401 from the *GOG API*, never from the
  CDN host itself.

- [ ] **Only per-chunk MD5 is verified; SHA-256 support is unused dead code.**
  `DepotFile.sha256` (`src/depot/depot_info.rs:28`) is parsed from the API but never read anywhere.
  `ChecksumAlgorithm::Sha256` (`src/downloader/util/hash.rs:15`) is fully implemented and marked
  `#[allow(dead_code)]` at line 14 — i.e. the crate already contains working SHA-256 verification that
  nothing calls. This looks like an unfinished feature (likely for GOG's whole-file-hash manifest
  entries) rather than intentional dead code.

## Medium — duplication & consistency

- [ ] **The "fetch → on 401 refresh → retry once" block is copy-pasted six times** nearly verbatim:
  `depot/depot_info.rs`, `depot/build_metadata.rs`, `games/owned_games.rs`, `games/game_details.rs`,
  `games/game_build.rs`, `secure_links/secure_links.rs`. This is exactly the kind of duplication that
  let the `refresh_auth` ordering bug above go unnoticed in one spot — extracting a shared helper would
  both remove ~150 lines and make the next auth-related fix apply everywhere at once.

- [ ] **"Not a game" handling is inconsistent across near-identical fetchers.**
  Only `GameDetails::get_game_details` (`src/games/game_details.rs:64-68`) turns a `DecodeError` into
  `GamesError::ProductNotAGame`. `GameBuilds`, `GameLinks`, `GameSummary`, and `OwnedGames` don't apply
  the same reasoning, so calling e.g. `get_game_builds` for a DLC/non-game product surfaces a raw
  `DecodeError` instead of the more meaningful `ProductNotAGame`.

- [ ] **Caching is inconsistent between depot data of similar shape.**
  `DepotManagerInner` caches `ProductDetails` (`src/depot/depot_manager.rs:22`) but has no cache for
  `DepotInfo` — the actual per-depot manifest, likely the largest payload fetched in the whole crate.
  `ProductBundle::get_download_files` (`src/downloader/product_bundle.rs`) re-fetches every depot's
  manifest from the CDN on every call, even for a build/product combination already resolved seconds
  earlier by `DownloadableProduct::get_downloadable_products`, which *is* cached.

- [ ] **Language and OS are hardcoded with no selection surface.**
  `BuildMetadata::filter_languages` is always called with `"en-US"`
  (`src/depot/build_metadata.rs:71`), and `GameBuilds::get_game_builds` always queries
  `os/windows/builds` (`src/games/game_build.rs:41`). There's currently no way for a caller to ask for
  a different language depot or a native Linux build. The OS choice may be intentional given Proton is
  used for everything, but it's worth documenting as a deliberate limitation rather than leaving it
  implicit.

- [ ] **`GameScreenshots::resolve_links` picks a magic formatter index.**
  `src/games/game_screenshots.rs:76-77` calls `formatter.get(2)` — the third available formatter —
  with no explanation of why index 2 specifically, and silently drops the screenshot entirely
  (`continue`) if fewer than 3 formatters are present instead of falling back to whatever's available.

## Medium — missing coverage

- [ ] **No automated tests anywhere in the crate.** Given the `restart` branch's explicit goal of a
  careful, from-scratch rebuild (per the workspace `CLAUDE.md`), this is worth addressing before the
  crate grows further rather than after.

- [ ] **No `CLAUDE.md`/`api.md` on the `restart` branch.** Both exist on `master` and are treated as
  canonical specs for consumers (`gogdl_flutter`); the `restart` branch currently has neither, so
  there's no single reference tracking what public surface has been rebuilt so far vs. still stubbed.

## Low — style / clippy

`cargo clippy --lib -- -W clippy::all` reports 34 warnings, mostly minor:
- [ ] Redundant `if let None = ... ` patterns instead of `.is_none()` in five files (`game_build.rs`,
  `game_details.rs`, `owned_games.rs`, `secure_links.rs`, and the `depot_info`/`build_metadata`
  equivalents).
- [ ] `OwnedGames::default()` is a hand-written inherent method that shadows/confuses with
  `std::default::Default` (`src/games/owned_games.rs:14`) — implement the trait instead.
- [ ] Module inception (`mod gogdl` inside `gogdl/mod.rs`, `mod secure_links` inside
  `secure_links/mod.rs`).
- [ ] A few needless `return`/`let`-then-return patterns in `downloader.rs` and `product_bundle.rs`.

Run `cargo clippy --fix --lib -p gogdl-lib -- -W clippy::all` to auto-apply most of these.
