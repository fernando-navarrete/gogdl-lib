# GAPS.md

Findings from a full read-through of `gogdl-lib` on the `restart` branch (all `src/*.rs`, `Cargo.toml`,
`cargo build`, `cargo clippy --all -- -W clippy::all`) on 2026-08-30. Grouped by severity, each item has
a checkbox so they can be worked one at a time. File:line references were accurate as of commit
`377314c`; the auth section and the auth-related call sites were re-checked against `a962ef7`
(`src/auth/model.rs` is now `src/auth/auth.rs`).

No automated tests exist yet (`grep -r '#\[test\]' src/` is empty), so treat every fix here as
untested until you add coverage for it.

---

## High — correctness bugs

- [x] **`refresh_auth` persists tokens without `valid_until`.** *Fixed in `998829d`.*
  `refresh_auth` no longer returns a JSON string at all (`Result<(), AuthError>`), and `valid_until`
  is now computed on the response *before* it is stored, so the ordering hazard is gone.

- [x] **Silent, internal token refreshes are invisible to callers.** *Addressed in `998829d` +
  `b87d8dd`.* `AuthManager` holds an optional `TokenObserver` (`src/auth/token_observer.rs`) that
  `refresh_auth` notifies, so the six internal 401-retry call sites (`depot/depot_info.rs`,
  `depot/build_metadata.rs`, `games/owned_games.rs`, `games/game_details.rs`, `games/game_build.rs`,
  `secure_links/secure_links.rs`) no longer refresh invisibly, and `b87d8dd` changed the callback to
  hand over the whole `Auth` (`src/auth/auth_manager.rs:79`), so the rotated refresh token reaches
  the app. `GogDl::refresh_auth()` was removed in `998829d`, so the observer is now the *only*
  channel by which an app can learn about a refresh — which makes the export blocker below a hard
  dependency rather than a nicety.

- [x] **`valid_until` is dead.** *Addressed in `998829d`* — `Auth::is_valid()`
  (`src/auth/auth.rs:26-29`) compares it against `chrono::Utc::now()`, and `AuthManager::get_auth`
  now gates on it. It is wired as a hard gate rather than a pre-flight refresh, which introduces two
  problems still open below (no proactive refresh, reachable `unwrap()` panics).

- [x] **Restoring persisted tokens locks the app out entirely.** *Fixed in `6c76f03`*, which dropped
  `#[serde(skip_deserializing)]` from `Auth::valid_until` (`src/auth/auth.rs:14`), so
  `restore_from_string` now recovers the stored expiry instead of always producing `None`. (`Option`
  fields default to `None` when absent, so the GOG API responses — which never carry `valid_until` —
  still deserialize, and both login and refresh overwrite it with a computed value.) A session
  restored *before* its access token expires now works. A session restored *after* expiry still fails
  — that case belongs to the next item.

- [ ] **Local expiry is a hard failure, not a refresh trigger.** *This is now the whole of the
  restart-lockout problem, and the common case.* `a962ef7` made the condition *legible* —
  `AuthManager::get_auth` (`src/auth/auth_manager.rs:84-98`) now returns
  `Result<Auth, AuthError>` and distinguishes `AuthError::AuthExpired` from
  `AuthError::Unauthorized` — but not *actionable*: all six call sites still convert either one
  straight into their own `AuthError(err)` and return, in a pre-flight check that runs before any
  request is made, so the 401 branch that would have refreshed is never reached. GOG access tokens
  live about an hour, so any relaunch after the app has been closed that long is still locked out
  with a valid refresh token sitting on disk, and with `GogDl::refresh_auth()` removed there is no
  way for the app to force a recovery. `get_auth` (or a new `get_valid_auth`) should refresh when the
  token is expired but a refresh token exists, and only then report `Unauthorized`.

- [x] **Token observer only emits the access token.** *Fixed in `b87d8dd`* —
  `TokenObserver::on_token_refreshed(&self, auth: Auth)` (`src/auth/token_observer.rs:4`) now hands
  over the full `Auth`, so the rotated refresh token and the computed `valid_until` are both
  available to persist. Blocked in practice by the export gap below.

- [ ] **`TokenObserver` cannot be implemented outside the crate.** `Auth` has no public path:
  `src/auth/mod.rs` declares `mod auth;` privately and re-exports only `AuthManager`, `AuthError`,
  and `TokenObserver`, and `src/lib.rs` re-exports only `TokenObserver`. Since `b87d8dd` the trait's
  one method names `Auth` in its signature, so a downstream crate cannot write the `impl` at all —
  verified: an external `impl gogdl_lib::TokenObserver` fails with ``E0425: cannot find type `Auth`
  in crate `gogdl_lib` ``. `Auth::to_string()` is unreachable for the same reason, so even a consumer
  that somehow held an `Auth` could not serialize it for `restore_auth`. The whole observer feature
  is dead from `gogdl_flutter`'s side until `Auth` is exported from `src/lib.rs` (and `to_string` /
  `from_string` along with it). This is the one gap that gates the token-persistence work as a whole.

- [ ] **The error detail added in `a962ef7` is invisible to consumers.** `src/lib.rs` exports
  `GogDlError` but none of the error types nested inside it — `AuthError`, `GamesError`,
  `DepotError`, `SecureLinksError`, `DownloadError` all live in private modules. A consumer can match
  `GogDlError::AuthError(_)` (binding with `_` needs no name) but cannot write
  `AuthError::AuthExpired`, so the `AuthExpired`/`Unauthorized` split that commit introduced is
  reachable only by string-matching `Display` output. It also can't reach the same condition arriving
  by the other route: an auth failure surfaces as either `GogDlError::AuthError(..)` or
  `GogDlError::GameError(GamesError::AuthError(..))` (and the `Depot`/`SecureLinks` equivalents)
  depending on which layer produced it, and the nested match can't be written at all. Export the
  error enums, or flatten auth failures to a single top-level variant.

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

- [ ] **Twelve `get_auth().await.unwrap()` calls became reachable panics in `998829d`.** Before that
  commit `get_auth()` failed only when no tokens were stored; now it also fails whenever the token is
  expired, which is a *time-dependent* condition. `a962ef7` changed the error type (`Option` →
  `Result`) but not the shape, so both duplicated forms are still unsound:
  - pre-flight: `if let Err(err) = lock.auth.get_auth().await { return Err(..) }` followed by a
    second `lock.auth.get_auth().await.unwrap()` (`games/owned_games.rs:27-30`,
    `games/game_details.rs:29-32`, `games/game_build.rs:35-38`, `depot/build_metadata.rs:33-36`,
    `depot/depot_info.rs:42-45`, `secure_links/secure_links.rs:40-43`) — two separate `await`s over
    the same mutex, with expiry able to flip between them. The `Result` now returned makes this a
    one-line fix at each site (bind the `Ok` instead of re-calling), so it is the cheapest of the
    open auth items.
  - post-refresh: `lock.auth.get_auth().await.unwrap()` (`games/owned_games.rs:48`,
    `games/game_details.rs:50`, `games/game_build.rs:59`, `depot/build_metadata.rs:53`,
    `depot/depot_info.rs:68`, `secure_links/secure_links.rs:63`) assumes a successful `refresh_auth`
    always leaves a valid token; a clock jump or a zero/negative `expires_in` from the API panics the
    process.

  Both go away with a single `get_valid_auth() -> Result<Auth, AuthError>` that checks once.

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

## Medium — auth concurrency & lifetime

- [ ] **The `inner` mutex is held across the refresh/login network round-trip.**
  `self.inner.lock().await.client.get_json::<Auth>(&url).await` (`src/auth/auth_manager.rs:42` and
  `:68`) keeps the guard alive for the whole statement, so every `get_auth()` in the crate — i.e.
  every in-flight request's pre-flight check — blocks for the duration of an HTTP call. `HttpClient`
  is `Clone` (`src/client/client.rs:11`), so the fix is to clone it out of the guard and drop the
  lock before awaiting. The six call sites make this worse by holding *their* manager's mutex across
  `refresh_auth()` too.

- [ ] **`refresh_lock` serializes refreshes but doesn't collapse them.**
  `refresh_auth` (`src/auth/auth_manager.rs:56-83`) takes `refresh_lock` and then unconditionally
  performs a refresh. With N concurrent requests hitting 401 together, all N queue on the lock and
  each fires its own refresh round-trip; because GOG rotates refresh tokens, each one invalidates the
  token the previous call just persisted, and the observer fires N times. After acquiring the lock,
  re-read the stored token and return early if it is already valid (i.e. someone else refreshed while
  we waited).

- [ ] **The observer callback runs while the `inner` mutex is held.**
  `observer.on_token_refreshed(...)` is invoked inside the `let mut inner = self.inner.lock().await`
  block (`src/auth/auth_manager.rs:75-81`). It's a synchronous callback into consumer code — over FFI
  into Dart, in this project's case — so a slow observer stalls all auth access and one that re-enters
  the crate (e.g. to make an API call while persisting) deadlocks. `b87d8dd` made this cheaper to fix:
  the callback now takes `Auth` by value, so the clone it receives can be made before the guard is
  taken and the guard dropped before calling out.

- [ ] **`is_valid()` has no clock-skew / in-flight margin.**
  `src/auth/auth.rs:26-29` accepts a token that expires one second from now, which will then 401
  mid-request. Subtract a margin (30–60s) so a token about to expire is refreshed up front.

- [ ] **`AuthManager::set_auth` is unreachable dead code.** `src/auth/auth_manager.rs:99-101` takes an
  `Auth`, but neither `Auth` nor `AuthManager` is exported from `src/lib.rs` (only `TokenObserver`
  is), and nothing in the crate calls it. Either drop it or keep it as the counterpart to the `Auth`
  export the observer now needs.

- [ ] **A registered `TokenObserver` can never be replaced with "none".**
  `set_token_observer` (`src/auth/auth_manager.rs:37-39`) only ever sets `Some`, so a consumer cannot
  detach on logout/teardown; the `Arc<dyn TokenObserver>` lives as long as the `AuthManager`.

## Medium — duplication & consistency

- [ ] **The "fetch → on 401 refresh → retry once" block is copy-pasted six times** nearly verbatim:
  `depot/depot_info.rs`, `depot/build_metadata.rs`, `games/owned_games.rs`, `games/game_details.rs`,
  `games/game_build.rs`, `secure_links/secure_links.rs`. This is exactly the kind of duplication that
  let the `refresh_auth` ordering bug above go unnoticed in one spot — and it is now the reason the
  reachable-`unwrap()` panic and the lock-held-across-refresh problem each exist in six places at
  once, and why `a962ef7` had to make the same edit six times to change one error type. Extracting a
  shared helper would both remove ~150 lines and make the next auth-related fix apply everywhere at
  once.

- [ ] **`DownloadError` was left out of the `a962ef7` error unification.** Five error enums now carry
  `AuthError(#[from] AuthError)` in place of a bare `Unauthorized` variant, but
  `src/downloader/error.rs:19-20` still declares `DownloadError::Unauthorized`, and `:56` still maps
  a 401 to it. So a 401 means two different things depending on which module produced it, and the
  `AuthExpired`/`Unauthorized` distinction stops at the downloader boundary. The downloader also has
  no 401-refresh-retry of its own — see the secure-links expiry item above, which is the same problem
  from the CDN side.

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
  crate grows further rather than after. The auth logic added in `998829d` is the clearest candidate
  to start with: `Auth::is_valid` boundaries, the `to_string`/`from_string` restore round-trip
  (regressed once already in `998829d`, fixed in `6c76f03`, and with no test to stop it happening
  again), and concurrent `refresh_auth` collapsing are all unit-testable without network access.

- [ ] **No `CLAUDE.md`/`api.md` on the `restart` branch.** Both exist on `master` and are treated as
  canonical specs for consumers (`gogdl_flutter`); the `restart` branch currently has neither, so
  there's no single reference tracking what public surface has been rebuilt so far vs. still stubbed.

## Low — style / clippy

`cargo clippy --lib -- -W clippy::all` reports 30 warnings as of `a962ef7` (34 at `377314c`, 37 at
`998829d`), mostly minor:
- [ ] `Auth::is_valid`'s `map_or(false, ...)` should be `is_some_and(...)` (`src/auth/auth.rs:27`).
- [x] Redundant `if let None = ... ` patterns instead of `.is_none()` in five files. *Fixed as a side
  effect of `a962ef7`* — those six pre-flight checks are now `if let Err(err) = ...`, which clears
  the whole `redundant_pattern_matching` family (the bulk of the 37 → 30 drop).
- [ ] `OwnedGames::default()` is a hand-written inherent method that shadows/confuses with
  `std::default::Default` (`src/games/owned_games.rs:14`) — implement the trait instead.
- [ ] Module inception (`mod gogdl` inside `gogdl/mod.rs`, `mod secure_links` inside
  `secure_links/mod.rs`).
- [ ] A few needless `return`/`let`-then-return patterns in `downloader.rs` and `product_bundle.rs`.

Run `cargo clippy --fix --lib -p gogdl-lib -- -W clippy::all` to auto-apply most of these.
