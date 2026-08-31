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

The third pass above landed as commit `9cd8d0a` ("Move authentication into HttpClient"), regressions and
all. Two follow-up commits the same day fix one regression outright and half-fix the other:
- `22bf182` ("Correct Fetch Authentication Parameters") swaps the two bool arguments back at exactly the
  four call sites this doc flagged (`game_build.rs`, `game_details.rs`, `owned_games.rs`,
  `secure_links.rs`), each now `.fetch(&url, false, true)` — the "four authenticated endpoints fetch with
  no auth token" item below is fixed, confirmed by re-reading all four call sites directly.
- `2ae200d` ("Remove secure links API and expose TokenObserver") does two unrelated things in one commit:
  it deletes `GogDl::get_secure_links` and `secure_links/mod.rs`'s `pub use secure_links::SecureLinks;`
  entirely (so `SecureLinks` is no longer emitted by any public method — a design choice, not a bug fix,
  but it does close the `SecureLinks` half of the "three types not exported" item below), and it adds
  `pub use client::TokenObserver;` to `src/lib.rs`. That second change was only a *half* fix for the
  export-regression item below — `Auth` itself wasn't re-exported yet, so `TokenObserver`'s
  `on_token_refreshed(&self, auth: Auth)` still couldn't be spelled out in an external `impl`.

A fourth commit, `3ed4dd0` ("Export Auth from the crate root"), closes that other half: `src/client/mod.rs`
now also has `pub use auth::Auth;`, and `src/lib.rs` has `pub use client::Auth;` alongside the existing
`TokenObserver` re-export. Confirmed fixed — a throwaway external test (`impl TokenObserver for Dummy {
fn on_token_refreshed(&self, _auth: gogdl_lib::Auth) {} }`) compiled cleanly against `3ed4dd0`, where the
same test previously failed with `E0425: cannot find type 'Auth'`. This closed the export-regression item
below in full: both halves (`Auth` and `TokenObserver`) are reachable from the crate root again, matching
the state `32e8786` originally established before `9cd8d0a` regressed it.

A fifth, uncommitted change staged on top of `3ed4dd0` is the biggest fix of the whole `9cd8d0a` lineage:
it rewrites `fetch`'s retry `match` (`src/client/client.rs:45-63`) to add a dedicated arm for
`Err(ClientError::AuthError(AuthError::TokenExpired))` that calls `refresh_auth` and loops again, and
splits the `HttpError{status,..}` arm so a non-401 status returns immediately with its original
`status`/`body` intact instead of being silently retried and discarded. Together these close two items
this doc has tracked since the very first `c7d39b9` pass: "local expiry is still a hard failure" (the
common "just launched after an hour" case now actually triggers a refresh, not just a real server 401)
and "any non-401 HTTP error gets silently retried and its detail discarded". That same change briefly
also added `FORBIDDEN` (403) alongside `UNAUTHORIZED` (401) as a refresh-triggering status; a further
uncommitted edit removed it again before this pass, so `fetch` now keys the refresh-and-retry path on
401 alone. That's the right call — refreshing on a permission-style 403 wouldn't help and would have
reintroduced the "detail discarded after pointless retries" shape this same change fixed for every
other non-401 status. Separately, `src/lib.rs` now re-exports every per-layer error enum (`AuthError`,
`ClientError`, `DepotError`, `DownloadError`, `GamesError`, `SecureLinksError`), closing the long-open
"error detail
invisible to consumers" item — confirmed with an external test matching all the way down to
`GogDlError::ClientError(ClientError::AuthError(AuthError::TokenExpired))`.

No automated tests exist yet (`grep -r '#\[test\]' src/` is empty), so treat every fix here as
untested until you add coverage for it.

---

## High — correctness bugs

- [x] **Four authenticated endpoints were fetching with no auth token at all.** *Fixed in `22bf182`
  ("Correct Fetch Authentication Parameters").* The two new `bool` parameters on `HttpClient::fetch`
  (`decode`, `require_auth`) had been swapped at four call sites when `9cd8d0a` changed the signature
  from `fetch(url, auth_manager: Option<AuthManager>, decode: bool)` to `fetch(url, decode: bool,
  require_auth: bool)`. `22bf182` corrects exactly the four flagged call sites, each now
  `.fetch(&url, false, true)` — confirmed by re-reading the current files:
  - `src/games/game_build.rs:38` (`GameBuilds::get_game_builds`)
  - `src/games/game_details.rs:29` (`GameDetails::get_game_details`)
  - `src/games/owned_games.rs:27` (`OwnedGames::get_owned_games`)
  - `src/secure_links/secure_links.rs:43` (`SecureLinks::get_secure_links`)

  All six authed fetchers (these four plus `build_metadata`/`depot_info`, which were already correct)
  now send the bearer token and route real 401s into `fetch`'s refresh-and-retry loop correctly. No test
  exists to pin this down, though — see the missing-coverage item below.

- [x] **`Auth` and `TokenObserver` were dropped from the crate root by `9cd8d0a`'s `src/auth/*` →
  `src/client/auth/*` move.** *Fixed in two steps: `TokenObserver` in `2ae200d`, `Auth` in an uncommitted
  follow-up.* The old `src/auth/mod.rs` (which had `pub use auth::Auth; pub use auth::TokenObserver;`,
  re-exported from `src/lib.rs`) no longer exists after the move, and nothing replaced the crate-root
  re-export for either type. `2ae200d` added `pub use client::TokenObserver;` back; a further uncommitted
  change adds `pub use auth::Auth;` to `src/client/mod.rs` and `pub use client::Auth;` to `src/lib.rs`.
  Confirmed fixed end-to-end with an external test: `impl TokenObserver for Dummy { fn
  on_token_refreshed(&self, _auth: gogdl_lib::Auth) {} }` now compiles against this tree (it previously
  failed on `Auth` with `E0425`, and before that on `TokenObserver` with `E0432`). This restores what
  `32e8786` originally established and `9cd8d0a` regressed — `gogdl_flutter`'s `TokenObserver` bridge
  wrapper should compile against this tree again.

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
  `b87d8dd`; re-broken by `9cd8d0a`, fixed again by the `Auth`/`TokenObserver` export item above.* `AuthManager` holds an optional `TokenObserver`
  (`src/client/auth/token_observer.rs`) that `refresh_auth` notifies
  (`src/client/auth/auth_manager.rs:86-93`), so the internal 401-retry path no longer refreshes
  invisibly, and the callback hands over the whole `Auth`, so the rotated refresh token reaches the app.
  `GogDl::refresh_auth()` doesn't exist, so the observer is the only channel by which an app can learn
  about a refresh — which is now usable again now that both types it needs are nameable outside the
  crate.

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

- [x] **Local expiry is finally a refresh-and-retry case, not a hard failure — the item this doc has
  tracked through every pass since `c7d39b9`.** *Fixed in the uncommitted change on top of `3ed4dd0`.*
  `fetch`'s retry `match` (`src/client/client.rs:45-63`) now has a dedicated arm,
  `Err(ClientError::AuthError(AuthError::TokenExpired)) => { self.auth_manager.refresh_auth(&self)
  .await?; }` (`:57-59`), that sits alongside the `HttpError{..}` arm rather than falling into the
  catch-all. Traced end to end: `inner_fetch`, when `require_auth` is true, calls
  `auth_manager.get_auth()`; on local expiry that returns `Err(AuthError::TokenExpired)`, wrapped as
  `ClientError::AuthError(AuthError::TokenExpired)`; `fetch`'s loop now matches that exact shape,
  refreshes, and loops back to retry `inner_fetch` — which re-checks `get_auth()`, now valid, and
  proceeds with the real request. The common "just launched after an hour" case is genuinely covered
  now, not just a real server 401. `AuthError::NotAuthenticated` (never logged in at all) correctly
  still falls to the catch-all instead of attempting a pointless refresh. `GogDl` still has no public
  `refresh_auth()`/pre-flight-refresh entry point, but that's no longer load-bearing now that `fetch`
  handles it internally on every call.

- [x] **The retry-with-refresh loop in `HttpClient::fetch` now catches both a real server 401 and local
  expiry, on every authed fetcher.** `fetch` (`src/client/client.rs:36-66`) matches
  `Err(ClientError::HttpError { status, body })` and, when `status == UNAUTHORIZED` and `require_auth` is
  true, calls `refresh_auth` before looping (`:47-56`); it also matches
  `Err(ClientError::AuthError(AuthError::TokenExpired))` directly (`:57-59`, see the item above). Since
  `22bf182` fixed the swapped `require_auth` arguments, all six authed fetchers reach both paths
  correctly. A brief detour through also matching `FORBIDDEN` (403) here was reverted before this pass
  landed — see the caveat on the item below for why that would have been the wrong call.

- [x] **New regression: any non-401 HTTP error was silently retried up to 3 times with its detail
  discarded.** *Fixed in the same uncommitted change as the item above.* The `HttpError{..}` arm
  (`src/client/client.rs:47-56`) now has an explicit `else { return Err(ClientError::HttpError { status,
  body: body }) }` for any status other than `UNAUTHORIZED` — a 404/403/500/etc. now fails fast with its
  original status and body preserved, exactly like before `c7d39b9` regressed this. Fully closed, no
  caveat: a brief version of this change also matched `FORBIDDEN` (403) into the refresh-and-retry path,
  which would have reintroduced this exact "detail discarded after pointless retries" shape for a
  genuine permission-denied 403 — that was reverted before landing, so a 403 now falls straight into
  this `else` and returns immediately, same as any other non-401 status.

- [x] **Token observer only emits the access token.** *Fixed in `b87d8dd`, unaffected by this pass* —
  `TokenObserver::on_token_refreshed(&self, auth: Auth)` (`src/client/auth/token_observer.rs:4`) still
  hands over the full `Auth`, so the rotated refresh token and the computed `valid_until` are both
  available to persist, and both are now actually usable outside the crate per the export item above.

- [x] **`TokenObserver` cannot be implemented outside the crate.** *Fixed in `32e8786`, re-broken by
  `9cd8d0a`, re-fixed in full by `2ae200d` + `3ed4dd0`.* Both `TokenObserver` and `Auth` are exported
  from `src/lib.rs` (see the item at the top of this section), so an external `impl TokenObserver for Foo
  { fn on_token_refreshed(&self, auth: Auth) { .. } }` compiles again — confirmed directly. `AuthError`
  is now exported too (see the error-detail item below), so a consumer's `TokenObserver` impl can also
  match on why a refresh happened, if it wants to. `AuthManager` alone remains crate-private — a narrow
  residual gap (a consumer can register an observer, receive `Auth`, and match `AuthError`, but still
  can't hold an `AuthManager` handle directly), which matters only if something wants manual control over
  auth state outside what `GogDl`'s own methods already expose.

- [x] **The error detail added in `a962ef7` was invisible to consumers.** *Fixed in the uncommitted
  change on top of `3ed4dd0`.* `src/lib.rs` now re-exports every per-layer error enum — `AuthError`,
  `ClientError`, `DepotError`, `DownloadError`, `GamesError`, `SecureLinksError` (`:30-35`) — alongside
  the `GogDlError` it already exported. Confirmed with an external test that matches all the way down
  through the full nesting: `GogDlError::ClientError(ClientError::AuthError(AuthError::TokenExpired))`,
  and each of `GogDlError::{GameError,DepotError,DownloadError,SecureLinksError}(XError::ClientError(_))`,
  all compile against this tree. `GogDlError::AuthError(_)` as a single top-level variant still doesn't
  exist (auth failures still arrive nested inside whichever layer produced them, as
  `XError::ClientError(ClientError::AuthError(..))`), so a consumer wanting "was this an auth problem"
  across every call still needs to match down through two enums rather than one flat variant — a real but
  much smaller ergonomics gap than "cannot name the type at all", which is what this item used to
  describe. `AuthManager` remains crate-private (unaffected by this pass) — see the `TokenObserver` item
  above for what that still blocks.

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
  `inner_fetch` makes internally (`src/client/client.rs:87-90`). Every authed request now does exactly
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

- [ ] **`refresh_lock` serializes refreshes but doesn't collapse them — and this is now the common-case
  path, not a corner case.** `refresh_auth` (`src/client/auth/auth_manager.rs:73-96`) still takes
  `refresh_lock` (`:75`) and then unconditionally performs a refresh with no early-return if another
  waiter already refreshed — unchanged. Reachability changed significantly this pass: now that local
  expiry itself drives a refresh-and-retry (see the fixed High-severity item above), *every* concurrent
  in-flight request issued right as the access token crosses its expiry boundary — the realistic,
  frequent scenario, not just a simultaneous-401 corner case — will race into `refresh_auth` at once. If
  N requests are in flight when the token expires, N of them take `refresh_lock` in turn and each
  performs its own full network round-trip refresh, rotating the refresh token N times in a row (the
  GOG API may itself only tolerate one live refresh token at a time, in which case requests 2..N could
  each invalidate the token the previous one just obtained). This is no longer a theoretical
  concurrency nit — it's the first thing worth testing under load now that the rest of the retry path
  works.

- [ ] **The observer callback runs while the `inner` mutex is held.**
  `observer.on_token_refreshed(...)` is invoked inside the `let mut inner = self.inner.lock().await`
  block (`src/client/auth/auth_manager.rs:88-93`), unchanged since `b87d8dd` beyond the file move. Same
  reachability upgrade as `refresh_lock` above — a slow or re-entrant `TokenObserver` (now genuinely
  implementable externally, per the fixed export item above) stalling every in-flight authed request at
  once is a real, common-case risk now, not a narrow theoretical one.

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
  cannot detach on logout/teardown; the `Arc<dyn TokenObserver>` lives as long as the `AuthManager`. Now
  practically reachable again — `TokenObserver`/`Auth` are both exported (see above), so this is a real,
  live gap for a consumer to hit, not a moot one.

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
  introduced the swapped-boolean-args regression (fixed in `22bf182`) and the local-expiry-bypass gap
  (fixed in the change on top of `3ed4dd0`, see the High-severity section) along the way; both are
  resolved now, so this has settled into a clean win in hindsight.

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
  (`src/client/client.rs:36-66`) is the single entry point for every JSON-returning request, and now that
  it correctly catches both local expiry and a real 401 (see the fixed High-severity items above), it's a
  genuinely solid funnel — but the CDN chunk transfer, `stream_chunk` (`:120-151`), the method that moves
  the actual multi-gigabyte payload and is where "No retry/backoff for transient network failures during
  download" (above) lives, is still a separate method entirely: no `AuthManager`, no retry loop, called
  directly by the downloader. None of this pass's fixes reach it — it needs its own, parallel
  retry/refresh implementation, or to be rewritten to go through `inner_fetch`/`fetch`.

- [ ] **Two types a consumer must be able to name are not exported — down from three.** The exported
  `ProductBundle` declares `pub product_files: Vec<DepotFile>` (`src/downloader/product_bundle.rs:8-10`);
  `DepotFile.chunks` (`src/depot/depot_info.rs:33`) is `Option<Vec<Chunk>>`. Neither `DepotFile` nor
  `Chunk` is re-exported from `src/lib.rs` — `998829d` dropped both (in favor of exporting
  `TokenObserver`) without narrowing what still *emits* them. A consumer that walks
  `ProductBundle.product_files` (which `verify_files`/`download_game` require holding onto, since
  `ProductBundle` isn't `Clone`) receives a `Vec` of a type it cannot name in a signature, `let` binding,
  or test fixture. `2ae200d` ("Remove secure links API and expose TokenObserver") closed the third leg of
  this item by removing `GogDl::get_secure_links` and `SecureLinks`'s re-export from
  `secure_links/mod.rs` entirely, rather than exporting the type — a bigger break for any consumer that
  was calling it (the method itself is gone, not just the return type unnameable), but it does mean
  `SecureLinks` is no longer a type a consumer needs to name at all. `lumen-cli`
  (`/home/fernando/repo/lumen-project/lumen-cli`, sibling repo in this workspace), which previously
  failed to compile with `E0432: unresolved imports gogdl_lib::DepotFile, gogdl_lib::SecureLinks`,
  actively calls `gog.get_secure_links(product_id)` at two call sites
  (`src/middleware/downloads.rs:131,200`) — its own doc comments there already note working around
  `SecureLinks` not being nameable (`downloads.rs:176-182`). Once `lumen-cli` bumps its `gogdl-lib` pin
  past this pass, both call sites become a hard `E0599: no method named 'get_secure_links'` compile
  break, not the type-naming workaround it currently has code for — this consumer needs either the
  method restored or a replacement API before that pin bump happens. Either re-export `DepotFile`/`Chunk`
  or narrow the public field that leaks them.

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
  both calls (unchanged by this pass), `inner_fetch` (`src/client/client.rs:81-97`) can *never* produce
  `ClientError::AuthError` on these paths — that branch only fires when `require_auth: true` is passed
  and the internal `get_auth()` fails locally. So every real failure from `AUTH_URL`/`REFRESH_URL` —
  including a 401 because the refresh token is dead, not just refreshable — falls straight to the
  stringified `ClientError { inner: String }` arm. That's `982dc82`'s exact failure mode back in a new
  shape: a dead-refresh-token 401 and a transient network blip are now both just an opaque string. `Auth`
  export work elsewhere in this pass (`AuthError` is now `pub` from the crate root — see the error-detail
  item above) means an external consumer can at least match `AuthError::ClientError { inner }` as its own
  variant now, distinguishing "some client-level failure happened during auth" from `TokenExpired`/
  `NotAuthenticated`/the decode errors — but `inner` is still just a `Display`-formatted string with no
  structure, so a dead refresh token and a transient network blip remain indistinguishable *within* that
  variant. Fix needs either a real status-aware variant on `AuthError` for this case, or at minimum
  removing the dead `Err(ClientError::AuthError(e))` arm so the code doesn't imply a distinction it can't
  actually make.

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

`cargo clippy --lib -- -W clippy::all` reports 34 warnings on the current (2026-08-31, sixth pass) staged
tree (34 at the still-uncommitted `403` version reviewed in the previous pass, 33 at `3ed4dd0`, 32 at
`43c1d85`, 31 at `c7d39b9`, 33 at `v0.0.4-restart`/`32e8786`, 30 at `a962ef7`, 34 at `377314c`, 37 at
`998829d`) — the 403-removal edit changed nothing about the warning count or categories, only shifted
three of them up by two lines as the function shrank. The net +1 from `3ed4dd0` still breaks down as:
**−1** "this `if` statement can be collapsed" — gone, because the nested `if status == UNAUTHORIZED { if
require_auth {..} }` shape it applied to no longer exists (the `if` got an `else` branch instead, per the
fixed non-401 item above); **+1** "redundant field names in struct initialization", now at
`src/client/client.rs:54` — `return Err(ClientError::HttpError { status, body: body })` should just be
`body`; **+1** "this expression creates a reference which is immediately dereferenced by the compiler",
now appearing *twice* instead of once (`src/client/client.rs:51` and `:58`) — the same
`self.auth_manager.refresh_auth(&self)` needless-borrow this doc already flagged, now duplicated because
the `AuthError::TokenExpired` arm makes the identical call a second time. Everything else below carries
forward unchanged in count from `3ed4dd0`; only file paths moved for anything under the old `src/auth/`,
now `src/client/auth/`:
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
  at `depot/depot_info.rs:44`, the pre-existing "redundant field names in struct initialization" at
  `downloader/download_unit.rs:33` (`offset: offset`, its new sibling at `client.rs:54` is called out
  above since it's part of this pass), and two `or_insert_with(Vec::new)` that should be `or_default()`
  (`downloader/downloadable_product.rs:50`, `downloader/product_bundle.rs:42`).

Run `cargo clippy --fix --lib -p gogdl-lib -- -W clippy::all` for a current full list and to
auto-apply most of these — but only against a clean tree; it will also try to "fix" whatever's
mid-edit if run against uncommitted work.
