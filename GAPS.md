# GAPS.md

Findings from a full read-through of `gogdl-lib` on the `restart` branch (all `src/*.rs`, `Cargo.toml`,
`cargo build`, `cargo clippy --all -- -W clippy::all`) on 2026-08-30. Grouped by severity, each item has
a checkbox so they can be worked one at a time. File:line references were originally accurate as of
commit `377314c`, then re-checked against `a962ef7`, then against `v0.0.4-restart` (`32e8786`), then
against the uncommitted changes staged on top of `32e8786` (`9a1f780` "Centralize HTTP requests through
typed request variants" rewrote all six fetchers and `auth_manager.rs`; that pass then deleted the
`Request` enum in favor of `HttpClient::fetch(url, auth_manager, decode)`, added the retry-with-refresh
loop inside `fetch`, and added a `ClientError::Unknown` variant mirrored onto all five other error
enums) — that whole sequence landed as commit `c7d39b9` ("Centralize HTTP Fetching With Retry Logic").

Re-checked again on 2026-08-31 against a further uncommitted rewrite staged on top of `c7d39b9`. This
pass renames things again (`ClientError::Http`→`HttpError`, `ClientError::Unknown`→`MaxRetriesReached`),
rewrites `AuthError` from scratch (`Unauthorized`→`NotAuthenticated`, `AuthExpired`→`TokenExpired`, the
old `UrlParseError`/`NetworkError`/`Http`/`DecodeError`/`DeflateError`/`Unknown` variants and the manual
`impl From<ClientError> for AuthError` are all gone, replaced by `AuthDecodeError`/`AuthEncodeError`/
`ClientError { inner: String }` and per-call-site `match`es in `auth_manager.rs`), and — the largest
change — deletes the four remaining hand-written `impl From<ClientError> for XError` blocks
(`Depot`/`Games`/`SecureLinks`/`DownloadError`) in favor of a single `ClientError(#[from] ClientError)`
variant on each. That whole sequence landed as commit `43c1d85` ("Refactor Client Errors And Retry
Handling").

Re-checked a third time, same day (2026-08-31), against a further uncommitted rewrite staged on top of
`43c1d85` (`git diff --cached`, nothing committed yet). This is a bigger structural move: `src/auth/*`
was relocated to `src/client/auth/*` (`git status` shows it as five renames), `AuthManager` moved from a
field each manager (`DepotManager`/`GamesManager`/`SecureLinksManager`/`DownloadManager`) carried
alongside its `HttpClient` to a field `HttpClient` owns internally — so `HttpClient::fetch`/
`fetch_no_retry` no longer take `Option<AuthManager>` at all, replaced by two `bool`s
(`decode`, `require_auth`), and every one of the six fetchers' duplicated "lock the manager, pre-flight
`get_auth()`, bail on `Err`, clone the manager, drop the lock" blocks is deleted outright rather than
just mechanically updated. That fixes the long-standing duplication item below — but the signature change
that made it possible introduced a severe new bug (four call sites pass the two new bools in the wrong
order) and the module move silently dropped two public re-exports that a downstream consumer
(`gogdl_flutter`) depends on. Both are written up as new top-of-list items below. Every reference in this
pass was re-derived with `grep -n`/`Read`/`cargo build`/`cargo clippy` against the current staged tree,
not carried forward from the `43c1d85` pass.

No automated tests exist yet (`grep -r '#\[test\]' src/` is empty), so treat every fix here as
untested until you add coverage for it — this pass in particular ships two regressions that any test
exercising `get_owned_games`/`get_game_details`/`get_game_builds`/`get_secure_links` against a real
token, or a mock `TokenObserver`, would have caught immediately.

---

## High — correctness bugs

- [ ] **New regression: four authenticated endpoints now fetch with no auth token at all**, because the
  two new `bool` parameters on `HttpClient::fetch` got swapped at the call site. The old signature was
  `fetch(url, auth_manager: Option<AuthManager>, decode: bool)`; the new one (`src/client/client.rs:32-37`)
  is `fetch(url, decode: bool, require_auth: bool)` — same two trailing slots, reordered. Two call sites
  that needed `decode: true` (`build_metadata.rs:32`, `depot_info.rs:47`, both `.fetch(url, true, true)`)
  translated correctly by accident, since both new args happen to be `true`. But the four call sites that
  needed `decode: false, require_auth: true` were mechanically rewritten to `.fetch(url, true, false)`
  instead of `.fetch(url, false, true)` — decode and require-auth swapped:
  - `src/games/game_build.rs:38` (`GameBuilds::get_game_builds`)
  - `src/games/game_details.rs:29` (`GameDetails::get_game_details`)
  - `src/games/owned_games.rs:27` (`OwnedGames::get_owned_games`)
  - `src/secure_links/secure_links.rs:43` (`SecureLinks::get_secure_links`)

  With `require_auth: false`, `inner_fetch` (`src/client/client.rs:73-94`) skips the auth branch
  entirely and calls plain `get_json` — no `Authorization: Bearer` header is sent, and the `decode: true`
  that was meant to matter is silently ignored too (the `if decode {}` branch only exists inside the
  `if require_auth {}` arm, so it's dead here). Every call to these four methods now hits a
  GOG-authenticated endpoint with no token. Compounding this: `fetch`'s retry loop
  (`src/client/client.rs:41-58`) only calls `refresh_auth` when `require_auth` is true (`:46-48`), so the
  resulting 401 doesn't even trigger a refresh — it just burns three identical unauthenticated attempts
  and returns the opaque `ClientError::MaxRetriesReached`, per the sibling item below. Net effect: owned
  games, game details, game builds, and secure links (i.e. everything needed to actually download or
  display a game) are all broken by this diff. Fix is a one-line swap at each of the four call sites.

- [ ] **New regression: `Auth` and `TokenObserver` are no longer exported from the crate root**,
  un-fixing two items marked resolved below. `src/auth/mod.rs` (which had `pub use auth::Auth;
  pub use auth::TokenObserver;` re-exported again from `src/lib.rs:10-11`) was deleted as part of the
  `src/auth/*` → `src/client/auth/*` move; the new `src/client/auth/mod.rs` still does
  `pub use auth::Auth; pub use token_observer::TokenObserver;`, but `mod client;` in `src/lib.rs:1` is
  private and nothing re-exports either type from the crate root any more — confirmed by writing a
  one-off external test (`use gogdl_lib::TokenObserver` / `gogdl_lib::Auth`) and getting
  `E0432: unresolved import` / `E0425: cannot find type`. `GogDl::set_token_observer`
  (`src/gogdl/gogdl.rs:132-134`) still takes `Arc<dyn TokenObserver>` and `GogDl::restore_auth` still
  round-trips through `Auth::to_string`/`from_string`, so the crate itself builds fine (`cargo build
  --lib` is clean) — this only breaks *consumers*. This silently reverts `32e8786` ("Export Auth publicly
  from the crate") and re-blocks the two items below marked fixed by it/`998829d`+`b87d8dd`
  ("`TokenObserver` cannot be implemented outside the crate", "Silent, internal token refreshes are
  invisible to callers") — `gogdl_flutter`'s bridge wrapper, which implements `TokenObserver` to persist
  rotated tokens, would fail to compile against this tree today. Fix: add
  `pub use client::auth::{Auth, TokenObserver};` (or equivalent) back to `src/lib.rs`.

- [x] **`refresh_auth` persists tokens without `valid_until`.** *Fixed in `998829d`, silently regressed
  by `9a1f780`, fixed again in the current uncommitted change.* `9a1f780`'s rewrite of `refresh_auth`
  to go through `client.fetch(Request::Get { url })` dropped the `auth.valid_until = Some(...)` line
  from the `Ok` branch it replaced — `git show 9a1f780 -- src/auth/auth_manager.rs` shows the
  assignment simply isn't carried into the new code, even though this very checkbox stayed marked
  fixed (nothing in the `32e8786` re-check caught it; the committed tree at `32e8786` genuinely
  refreshes tokens with `valid_until: None`, which `is_valid()` then reads as immediately expired).
  The current uncommitted change re-adds it: `refresh_auth` now calls `client.fetch_no_retry(&url,
  None, false)` and then sets `auth.valid_until = Some(auth.expires_in as i64 +
  chrono::Utc::now().timestamp())` (`src/auth/auth_manager.rs:78-79`) before storing, matching
  `login_with_code`'s existing `:48-50`. Worth a regression test given it has now broken twice across
  refactors with no automated coverage to catch it — see the missing-coverage item below.

- [x] **Silent, internal token refreshes are invisible to callers.** *Addressed in `998829d` +
  `b87d8dd`, re-broken by the export regression at the top of this section.* `AuthManager` holds an
  optional `TokenObserver` (`src/client/auth/token_observer.rs`) that `refresh_auth` notifies
  (`src/client/auth/auth_manager.rs:86-93`), so the internal 401-retry path no longer refreshes
  invisibly, and the callback hands over the whole `Auth`, so the rotated refresh token reaches the app
  *in principle*. `GogDl::refresh_auth()` doesn't exist, so the observer is the only channel by which an
  app can learn about a refresh — which is exactly why the "`Auth`/`TokenObserver` no longer exported"
  item above is a hard regression of this one, not a separate concern: a consumer can no longer name
  either type needed to register the observer at all.

- [x] **`valid_until` is dead.** *Addressed in `998829d`* — `Auth::is_valid()`
  (`src/client/auth/auth.rs:32-35`) compares it against `chrono::Utc::now()`, and `AuthManager::get_auth`
  now gates on it. It is wired as a hard gate rather than a pre-flight refresh, which introduces two
  problems still open below (no proactive refresh, reachable `unwrap()` panics).

- [x] **Restoring persisted tokens locks the app out entirely.** *Fixed in `6c76f03`*, which dropped
  `#[serde(skip_deserializing)]` from `Auth::valid_until` (`src/client/auth/auth.rs:14`), so
  `restore_from_string` now recovers the stored expiry instead of always producing `None`. (`Option`
  fields default to `None` when absent, so the GOG API responses — which never carry `valid_until` —
  still deserialize, and both login and refresh overwrite it with a computed value.) A session
  restored *before* its access token expires now works. A session restored *after* expiry still fails
  — that case belongs to the next item.

- [ ] **Local expiry is still a hard failure in practice — the bug moved, it didn't close.** *Restated
  against the 2026-08-31 `client/auth` move.* The six fetchers' duplicated pre-flight `get_auth()` blocks
  described in earlier passes of this doc are now genuinely gone (see the duplication item below, now
  marked fixed) — every fetcher calls `.fetch(url, decode, true)` directly and lets `inner_fetch` do the
  one `auth_manager.get_auth()` call. But that didn't fix the underlying bug, it just relocated it:
  `inner_fetch` (`src/client/client.rs:73-94`) maps a local-expiry failure to
  `Err(ClientError::AuthError(err))` (`:82`), and `fetch`'s retry loop (`:38-58`) only special-cases
  `Err(ClientError::HttpError { status, body })` (`:43`) — `ClientError::AuthError` falls straight into
  the catch-all `Err(err) => return Err(err)` arm (`:52-54`) with no refresh attempt and no retry. So a
  token that's expired *locally* (the common "just launched after an hour" case, `AuthManager::get_auth`
  at `src/client/auth/auth_manager.rs:99-110` returning `AuthError::TokenExpired`) still ends the request
  immediately, same as every prior pass of this doc found — only the file/line has moved. `GogDl` still
  has no public `refresh_auth()`/pre-flight-refresh entry point either. Fixing this needs `fetch`'s match
  to also treat `ClientError::AuthError(AuthError::TokenExpired)` as a refresh-and-retry case, not just
  a real server `HttpError{status: 401}`.

- [ ] **The retry-with-refresh loop in `HttpClient::fetch` catches a real server 401, but the
  local-expiry half above is still bypassed, and it's now reachable through a second, buggier path
  too.** `fetch` (`src/client/client.rs:32-58`) matches `Err(ClientError::HttpError { status, body })`
  (`:43-51`) and, when `status == StatusCode::UNAUTHORIZED` *and* `require_auth` is true, calls
  `self.auth_manager.refresh_auth(&self).await?` (`:46-48`) before looping again — so a genuine
  server-issued 401 on a correctly-flagged call is retried after a refresh. But per the item above, this
  is still the *only* thing that refreshes — local expiry never reaches it. And per the new
  boolean-argument-swap item at the top of this section, four call sites now pass `require_auth: false`
  on endpoints that actually need auth, so even a real 401 from *those* four never triggers a refresh
  either — it just silently retries unauthenticated 3 times (see the `MaxRetriesReached` item below).

- [ ] **New regression: any non-401 HTTP error now gets silently retried up to 3 times and its detail
  discarded.** In the same match arm (`src/client/client.rs:43-51`), a non-401 status — 404, 403, 500,
  anything — still falls into the `Err(ClientError::HttpError { status, body })` arm (the `if` only
  gates the refresh call, not the branch itself), so `status`/`body` are bound, thrown away
  (`let _ = body;`, `:44`), and the loop just `continue`s with no backoff. After 3 pointless identical
  attempts, `fetch` returns the generic `ClientError::MaxRetriesReached` (`:57`) instead of the original
  status/body. Concretely: a request for a nonexistent depot/product manifest that used to fail fast
  with a `404` + body now burns two extra network round-trips and then reports an uninformative "Max
  retires reached" with no indication it was ever a 404 — and, per the boolean-swap item at the top of
  this section, this is now also what a real 401 looks like on the four broken endpoints, since they
  never trigger the refresh branch. Fix needs the `if status == UNAUTHORIZED { .. } else { return
  Err(..) }` split reinstated inside that arm.

- [x] **Token observer only emits the access token.** *Fixed in `b87d8dd`, unaffected by this pass* —
  `TokenObserver::on_token_refreshed(&self, auth: Auth)` (`src/client/auth/token_observer.rs:4`) still
  hands over the full `Auth`, so the rotated refresh token and the computed `valid_until` are both
  available to persist. Blocked in practice by the export regression at the top of this section.

- [x] **`TokenObserver` cannot be implemented outside the crate.** *Fixed in `32e8786`, re-broken by the
  export regression at the top of this section.* `src/client/auth/mod.rs` still re-exports `Auth` and
  `TokenObserver` *within the crate*, and the trait's `on_token_refreshed(&self, auth: Auth)` signature
  is unchanged — but neither type reaches `src/lib.rs` any more (see that item for the exact break), so
  this is functionally unfixed again despite no code in this trait or its surrounding module having
  regressed. `AuthManager` and `AuthError` remain crate-private on top of that, unchanged from before —
  see the error-export item below.

- [ ] **The error detail added in `a962ef7` is invisible to consumers — and one rung of nesting deeper
  than before.** `src/lib.rs` exports `GogDlError` but none of the error types nested inside it —
  `AuthError`, `GamesError`, `DepotError`, `SecureLinksError`, `DownloadError`, and now `ClientError`
  too, all live in private modules. Previously a consumer could at least match the top-level
  `GogDlError::AuthError(_)` variant to distinguish "it was an auth problem" without naming `AuthError`
  itself; this pass deleted that variant (`src/gogdl/error.rs` no longer has `AuthError(#[from]
  AuthError)`, only `ClientError(#[from] ClientError)`, since `AuthError` no longer reaches `GogDlError`
  by any other route either — the four per-type error enums dropped their own `AuthError` variants in
  the same pass, see the Medium duplication section). So today every auth failure surfaces as
  `GogDlError::ClientError(..)`, indistinguishable at the top level from a URL-parse error, a network
  error, or a JSON-decode error — a consumer can no longer even do the coarse `_`-binding match this item
  previously described as still possible. `AuthError` itself is `NotAuthenticated`/`TokenExpired`/
  `AuthDecodeError`/`AuthEncodeError`/`ClientError { inner: String }`
  (`src/client/auth/error.rs`) — none of it nameable outside the crate. Export the error enums, or
  flatten auth failures to a single top-level `GogDlError` variant again (this time keeping it as
  failures actually arrive, i.e. via `ClientError::AuthError`, not reintroducing the old direct variant
  that bypassed `ClientError`).

## High — panics on untrusted data

- [ ] **Two `.parse().unwrap()` calls can crash the process on bad input**, with no handling for the
  identical failure a few lines away in a sibling file:
  - `src/secure_links/links_manager.rs:46` — `game_id.parse().unwrap()`, where `game_id: &str` is a
    public API parameter (ultimately comes from the Flutter/Dart side across FFI). A malformed
    caller-supplied id panics the whole process rather than returning an error.
  - `src/downloader/downloadable_product.rs:61` — `product_id.parse().unwrap()` on a value taken from
    the GOG API response.
  - Compare with `src/downloader/product_bundle.rs:52`, which parses the *same kind* of value with
    `.parse().unwrap_or(0)` — i.e. the two near-duplicate code paths (`get_downloadable_products` vs
    `get_download_files`) handle the same possible failure inconsistently, one by panicking and one by
    silently substituting `0`.

- [x] **Twelve `get_auth().await.unwrap()` calls became reachable panics in `998829d`.** *Fixed in
  `9a1f780`, and the residual double-`get_auth()` cost noted in the previous pass is now also gone.* —
  `grep -rn 'get_auth().await.unwrap()' src/` is still empty, and this pass deleted the six fetchers'
  pre-flight `get_auth()` blocks entirely (see the duplication item below) rather than just binding their
  `Ok` — so there's no longer a redundant pre-flight call being paid for on top of the one
  `inner_fetch` makes internally (`src/client/client.rs:80-83`). Every authed request now does exactly
  one `get_auth()` await over the mutex, down from two.

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
  `SecureLinksManager::links_cache` (`src/secure_links/links_manager.rs:21,44-56`) never evicts an
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

- [x] **The `inner` mutex is held across the refresh/login network round-trip.** *Fixed in `9a1f780`,
  unaffected by this pass beyond the file move.* `login_with_code` (`src/client/auth/auth_manager.rs:
  50-64`) and `refresh_auth` (`:73-96`) each take their tokens/refresh-token out of the guard, drop the
  lock, and only then await the request via `client.fetch_no_retry(...)`.

- [ ] **`refresh_lock` serializes refreshes but doesn't collapse them.** `refresh_auth`
  (`src/client/auth/auth_manager.rs:73-96`) still takes `refresh_lock` (`:75`) and then unconditionally
  performs a refresh with no early-return if another waiter already refreshed — unchanged. Reachability
  narrowed again by this pass: `AuthManager` now lives inside `HttpClient` and is called from `fetch`'s
  retry loop only on a real `ClientError::HttpError{status: 401}` with `require_auth: true`
  (`src/client/client.rs:43-49`) — per the High-severity items above, local expiry never reaches this at
  all, and now neither do the four fetchers with the swapped `require_auth` argument. In practice the
  only calls left that can drive concurrent refreshes today are `build_metadata`'s and `depot_info`'s
  (the two correctly-wired authed fetchers) hitting a genuine server 401 at the same time. Re-verify once
  local expiry actually reaches this path, since that's the common case that would exercise it.

- [ ] **The observer callback runs while the `inner` mutex is held.**
  `observer.on_token_refreshed(...)` is invoked inside the `let mut inner = self.inner.lock().await`
  block (`src/client/auth/auth_manager.rs:88-93`), unchanged since `b87d8dd` beyond the file move. Same
  narrowed-reachability note as `refresh_lock` above.

- [ ] **`is_valid()` has no clock-skew / in-flight margin.**
  `src/client/auth/auth.rs:32-35` accepts a token that expires one second from now, which will then 401
  mid-request. Subtract a margin (30–60s) so a token about to expire is refreshed up front.

- [x] **`AuthManager::set_auth` is still unreachable, but for a narrower reason now.** *Resolved by
  removal.* The method no longer exists — `grep -n 'fn set_auth' src/` is empty, and the current
  `src/client/auth/auth_manager.rs` only exposes `new`/`get_login_url`/`set_token_observer`/
  `login_with_code`/`restore_from_string`/`refresh_auth`/`get_auth`. Whether that was a deliberate
  cleanup or a side effect of the rewrite, the dead/unreachable method this item flagged is gone either
  way.

- [ ] **A registered `TokenObserver` can never be replaced with "none".**
  `set_token_observer` (`src/client/auth/auth_manager.rs:29-31`) only ever sets `Some`, so a consumer
  cannot detach on logout/teardown; the `Arc<dyn TokenObserver>` lives as long as the `AuthManager`. (In
  practice currently moot: per the export-regression item above, no external consumer can name
  `TokenObserver` to register one at all right now.)

## Medium — duplication & consistency

- [x] **The "fetch → on 401 refresh → retry once" block used to be copy-pasted six times.** *Fixed by
  this pass* — the six near-identical pre-flight blocks (lock the manager, `get_auth().await` and bail
  on `Err`, clone the manager, drop the lock) are gone outright, not just mechanically updated:
  `depot_info.rs`, `build_metadata.rs`, `owned_games.rs`, `game_details.rs`, `game_build.rs`, and
  `secure_links.rs` all now call `.fetch(url, decode, require_auth)` directly with no local `AuthManager`
  in scope at all (the managers dropped their own `auth: AuthManager` field — see `depot_manager.rs`,
  `games_manager.rs`, `links_manager.rs`, `download_manager.rs`). The double-`get_auth`-per-request cost
  this item used to note is gone with it (see the fixed panic item above). The mechanism this closed
  duplication with — folding auth into `HttpClient` itself and gating on a `bool` — is also what
  introduced the two new top-of-High-severity-section regressions (the swapped boolean args, and local
  expiry never reaching the retry loop from its new location), so this is a real fix with real fallout,
  not a clean win.

- [x] **`DownloadError` was left out of the `a962ef7` error unification.** *Fixed in `9a1f780`,
  restructured again by this pass.* `DownloadError::Unauthorized` is gone; `src/downloader/error.rs`
  carried `AuthError(#[from] AuthError)` through `43c1d85`, and this pass removed that variant too
  (matching `Depot`/`Games`/`SecureLinks`Error, see the dead-variant finding below) since `AuthError` can
  no longer reach `DownloadError` by any route other than already being wrapped in `ClientError`. Only
  `ClientError(#[from] ClientError)` remains for both concerns. The second half of the original item is
  still open, restated below as its own Medium finding — the downloader's transfer path
  (`stream_chunk`) still has no 401-refresh-retry of its own.

- [ ] **"Not a game" handling is inconsistent across near-identical fetchers.**
  Only `GameDetails::get_game_details` (`src/games/game_details.rs:17-24`, line numbers shifted down by
  the pre-flight-auth-block removal in this pass) turns a cached `None` into `GamesError::ProductNotAGame`.
  `GameBuilds`, `GameLinks`, `GameSummary`, and `OwnedGames` don't apply the same reasoning, so calling
  e.g. `get_game_builds` for a DLC/non-game product surfaces a raw `DecodeError` instead of the more
  meaningful `ProductNotAGame`.

- [ ] **Caching is inconsistent between depot data of similar shape.**
  `DepotManagerInner` caches `ProductDetails` (`src/depot/depot_manager.rs:20`) but has no cache for
  `DepotInfo` — the actual per-depot manifest, likely the largest payload fetched in the whole crate.
  `ProductBundle::get_download_files` (`src/downloader/product_bundle.rs`) re-fetches every depot's
  manifest from the CDN on every call, even for a build/product combination already resolved seconds
  earlier by `DownloadableProduct::get_downloadable_products`, which *is* cached.

- [ ] **Language and OS are hardcoded with no selection surface.**
  `BuildMetadata::filter_languages` is always called with `"en-US"`
  (`src/depot/build_metadata.rs:34`), and `GameBuilds::get_game_builds` always queries
  `os/windows/builds` (`src/games/game_build.rs:34`). There's currently no way for a caller to ask for
  a different language depot or a native Linux build. The OS choice may be intentional given Proton is
  used for everything, but it's worth documenting as a deliberate limitation rather than leaving it
  implicit.

- [ ] **`GameScreenshots::resolve_links` picks a magic formatter index.**
  `src/games/game_screenshots.rs:76-77` calls `formatter.get(2)` — the third available formatter —
  with no explanation of why index 2 specifically, and silently drops the screenshot entirely
  (`continue`) if fewer than 3 formatters are present instead of falling back to whatever's available.

- [ ] **`HttpClient::stream_chunk` bypasses the `fetch`/`inner_fetch` funnel entirely.** `fetch`
  (`src/client/client.rs:31-54`) is the single entry point for every JSON-returning request (and now
  the home of the new retry-with-refresh loop discussed above), but the CDN chunk transfer —
  `stream_chunk` (`:94-123`), the method that moves the actual multi-gigabyte payload and is where "No
  retry/backoff for transient network failures during download" (above) lives — is a separate method
  entirely, taking no `AuthManager` at all and called directly by the downloader. Even once `fetch`'s
  retry loop is made to actually catch local expiry and real 401s (per the High-severity item above),
  it still won't reach `stream_chunk` — that needs its own, parallel retry/refresh implementation, or
  to be rewritten to go through `inner_fetch`/`fetch`.

- [ ] **Three types a consumer must be able to name are not exported.** `GogDl::get_secure_links`
  (`src/gogdl/gogdl.rs:80`) returns `SecureLinks`; the exported `ProductBundle` declares
  `pub product_files: Vec<DepotFile>` (`src/downloader/product_bundle.rs:8-10`); `DepotFile.chunks`
  (`src/depot/depot_info.rs:33`) is `Option<Vec<Chunk>>`. None of `SecureLinks`, `DepotFile`, `Chunk`
  are re-exported from `src/lib.rs` — `998829d` dropped `Chunk`/`DepotFile` (in favor of exporting
  `TokenObserver`) and `0418bc0` dropped `SecureLinks` outright, both without narrowing what still
  *emits* them. A consumer that calls `get_secure_links` cannot bind its result to anything but an
  immediately-consumed temporary; one that walks `ProductBundle.product_files` (which
  `verify_files`/`download_game` require holding onto, since `ProductBundle` isn't `Clone`) receives a
  `Vec` of a type it cannot name in a signature, `let` binding, or test fixture. Live proof: `lumen-cli`
  (`ssh://git@thinkcentre.home:2200/gogdl/lumen-cli.git`, this workspace) fails to compile against
  `v0.0.4-restart` with exactly this — `E0432: unresolved imports gogdl_lib::DepotFile,
  gogdl_lib::SecureLinks` — because its manifest-walking and CDN-URL-resolution code (production code,
  not just the test fixtures that also construct `Chunk` directly) names both. Either re-export all
  three or narrow the public methods/fields that leak them.

- [ ] **The `982dc82` collapsing regression has effectively recurred: a dead refresh token and a
  routine expiry now both surface as the same opaque, unstructured error.** This item used to describe
  a *risk* — `ClientError::AuthError(#[from] AuthError)` wrapping one way and a manual `impl
  From<ClientError> for AuthError` unwrapping the other, at the exact seam where `982dc82` previously
  collapsed "expired access token" and "dead refresh token" into an indistinguishable `Unauthorized`
  (per `lumen-cli/CLAUDE.md`'s "Auth refresh regression" section). The rewrite that landed as `43c1d85`
  deleted that `impl From<ClientError> for AuthError` entirely (it no longer exists in
  `src/client/auth/error.rs`) and replaced it with ad hoc `match`es at the two call sites in
  `auth_manager.rs` (`login_with_code:57-64`, `refresh_auth:80-87`, current line numbers after the
  `client/auth` move): both call `client.fetch_no_retry(&url, false, false)`, match
  `Err(ClientError::AuthError(e)) => return Err(e)` first, and otherwise fall to `Err(e) =>
  AuthError::ClientError { inner: e.to_string() }`. But because `require_auth` is hardcoded to `false` in
  both calls (unchanged by this pass), `inner_fetch` (`src/client/client.rs:79-83`) can *never* produce
  `ClientError::AuthError` on these paths — that branch only fires when `require_auth: true` is passed
  and the internal `get_auth()` fails locally. So every real failure from `AUTH_URL`/`REFRESH_URL` —
  including a 401 because the refresh token is dead, not just refreshable — falls straight to the
  stringified `ClientError { inner: String }` arm. That's `982dc82`'s exact failure mode back in a new shape: a dead-refresh-token 401 and
  a transient network blip are now both just an opaque string a caller can only distinguish by parsing
  `Display` output, with no `AuthExpired`/`Unauthorized`-style variant to match on at all. Fix needs
  either a real status-aware variant on `AuthError` for this case, or at minimum removing the dead
  `Err(ClientError::AuthError(e))` arm so the code doesn't imply a distinction it can't actually make.

- [ ] **`Http`/`UrlParseError`/`NetworkError`/`DecodeError`/`DeflateError` on `DepotError`, `GamesError`,
  `SecureLinksError`, and `DownloadError` are now dead code.** The `43c1d85` rewrite deleted the four
  hand-written `impl From<ClientError> for XError` blocks that used to translate a `ClientError` into
  one of these per-type variants (e.g. `ClientError::Http{status: 401,..}` → `DepotError::AuthError(...)`,
  everything else → the matching `Depot`-flavored variant), replacing each with a single blanket
  `ClientError(#[from] ClientError)` variant. This pass additionally removed the now-redundant
  `AuthError(#[from] AuthError)` variant each of the four enums had kept alongside `ClientError` (see
  the "`DownloadError` was left out" item above), but left the five-per-enum dead variants untouched. But
  every network call in `depot/`, `games/`, and
  `secure_links/` already goes exclusively through `HttpClient` (confirmed: `grep -rn
  "reqwest::\|url::Url::parse\|serde_json::from_str" src/depot src/games src/secure_links` matches
  nothing outside `error.rs` itself), and `downloader/` is the same except for its own directly-
  constructed `DeflateError` (`src/downloader/downloader.rs:200,204`). So with the translation gone,
  `.fetch()?` now always arrives as `XError::ClientError(..)`, and nothing in the crate can construct
  `DepotError::Http`/`UrlParseError`/`NetworkError`/`DecodeError`/`DeflateError` (or the `Games`/
  `SecureLinks`/`Download` equivalents, `DeflateError` excepted for `Download`) anymore — confirmed with
  `grep -rn "::Http {" src/` and the equivalent per-variant greps, all empty. Because these enums are
  `pub`, `dead_code` doesn't warn on them, so this won't show up in `cargo clippy`. Either delete the
  now-unreachable variants (and their `use reqwest::StatusCode`/`use std::io` imports, e.g.
  `src/depot/error.rs:1,3`) or reinstate a translation that actually uses them — leaving both a working
  wrapper *and* five dead siblings per enum is confusing surface for whoever reads these error types
  next.

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

`cargo clippy --lib -- -W clippy::all` reports 33 warnings on the current (2026-08-31, third pass)
staged tree (32 at `43c1d85`, 31 at `c7d39b9`, 33 at `v0.0.4-restart`/`32e8786`, 30 at `a962ef7`, 34 at
`377314c`, 37 at `998829d`). The +1 from `43c1d85` is a single new hit inside this diff's own code —
"this expression creates a reference which is immediately dereferenced by the compiler" at
`src/client/client.rs:47`, on `self.auth_manager.refresh_auth(&self).await?` inside `fetch`'s retry arm
(`self` is already `&HttpClient`, so `&self` is a redundant double reference; clippy suggests just
`self`). The pre-existing "this `if` statement can be collapsed" hit survives at the same nested
`if status == UNAUTHORIZED { if require_auth { ... } }` shape, now at `src/client/client.rs:45-49`
instead of `:41`. Everything else below carries forward unchanged in count from `43c1d85`; only file
paths moved for anything under the old `src/auth/`, now `src/client/auth/`:
- [x] Redundant `if let None = ...` patterns instead of `.is_none()` in five files. *Fixed as a side
  effect of `a962ef7`.*
- [ ] `Auth::is_valid`'s `map_or(false, ...)` should be `is_some_and(...)`
  (`src/client/auth/auth.rs:33-34`, moved from `src/auth/auth.rs`).
- [ ] `OwnedGames::default()` is a hand-written inherent method that shadows/confuses with
  `std::default::Default` (`src/games/owned_games.rs:14-16`) — implement the trait instead.
- [ ] The "useless use of `format!`" on `format!("https://embed.gog.com/user/data/games")` with no
  interpolation is still there (`src/games/owned_games.rs:25`, shifted up from `:32` now that the
  pre-flight auth block above it is gone).
- [ ] 6 "returning the result of a `let` binding from a block" (`downloader/download_manager.rs:74,89`,
  `downloader/downloader.rs:283,371,459`, `downloader/product_bundle.rs:47`) and 4 "redundant
  redefinition of a binding `path_resolver`" (`downloader/downloader.rs:158,235,294,382`, all the same
  `let path_resolver = path_resolver;` idiom moving it into a closure) — the bulk of the count,
  unchanged since at least `a962ef7`, both confined to the downloader (untouched by this pass).
- [ ] Module inception, still 5: `mod auth` (now nested as `client::auth::auth`, same shape as before
  under a new parent), `mod client`, `mod downloader`, `mod gogdl`, `mod secure_links`.
- [ ] 1 "all variants have the same postfix: `Error`" on `FileSystemError`
  (`downloader/fs/error.rs:6-21`, `enum_variant_names`) — `ClientError` still isn't flagged for this
  despite still having a non-`Error`-suffixed variant among its six (`MaxRetriesReached`).
- [ ] A handful of smaller one-offs, mostly untouched by this pass: an explicit-closure-for-cloning in
  `depot/build_metadata.rs:38-46`, a `len_zero` at `downloader.rs:82`, two `redundant_closure`s
  (`downloader.rs:115,200`), `io_other_error` at `util/hash.rs:66`, a redundant `&` in a `format!` call
  at `depot/depot_info.rs:44`, one "redundant field names in struct initialization" at
  `downloader/download_unit.rs:33` (`offset: offset`), and two `or_insert_with(Vec::new)` that should be
  `or_default()` (`downloader/downloadable_product.rs:50`, `downloader/product_bundle.rs:42`).

Run `cargo clippy --fix --lib -p gogdl-lib -- -W clippy::all` for a current full list and to
auto-apply most of these — but only against a clean tree; it will also try to "fix" whatever's
mid-edit if run against uncommitted work.
