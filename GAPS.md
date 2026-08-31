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
variant on each. Most line numbers moved again; every reference below was re-derived with `grep -n`/
`Read`/`cargo build` against the current staged tree, not carried forward.

No automated tests exist yet (`grep -r '#\[test\]' src/` is empty), so treat every fix here as
untested until you add coverage for it.

---

## High — correctness bugs

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
  `b87d8dd`.* `AuthManager` holds an optional `TokenObserver` (`src/auth/token_observer.rs`) that
  `refresh_auth` notifies, so the six internal 401-retry call sites (`depot/depot_info.rs`,
  `depot/build_metadata.rs`, `games/owned_games.rs`, `games/game_details.rs`, `games/game_build.rs`,
  `secure_links/secure_links.rs`) no longer refresh invisibly, and `b87d8dd` changed the callback to
  hand over the whole `Auth` (`src/auth/auth_manager.rs:85`), so the rotated refresh token reaches
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

- [ ] **Local expiry is still a hard failure in practice, despite the new retry loop below.**
  `AuthManager::get_auth` (`src/auth/auth_manager.rs:90-104`) still distinguishes
  `AuthError::AuthExpired` from `AuthError::Unauthorized`, and the six fetchers (`depot/depot_info.rs`,
  `depot/build_metadata.rs`, `games/owned_games.rs`, `games/game_details.rs`, `games/game_build.rs`,
  `secure_links/secure_links.rs`) each still run their own pre-flight `lock.auth.get_auth().await` and
  `return Err(XError::AuthError(err))` *before ever calling `.fetch()`* — unchanged by the current
  diff. So an expired token still ends the session on the very first fetcher call after relaunch, and
  the new retry-with-refresh loop added to `HttpClient::fetch` (see below) never even gets invoked for
  this, the common case (GOG access tokens live about an hour). Verified live: `lumen-cli`'s
  `Session::refresh` (`middleware/mod.rs:94`, written against an earlier `gogdl-lib` checkout) calls
  `self.inner.refresh_auth()` on `GogDl` and fails to compile against `v0.0.4-restart` —
  `GogDl::refresh_auth()` has not existed since `998829d`, and nothing replaced it in the wrapper.
  Fixing this needs the pre-flight checks themselves to attempt a refresh (or to be removed so the
  call reaches `fetch`'s loop at all), not just retry logic added downstream of them.

- [ ] **The retry-with-refresh loop in `HttpClient::fetch` now catches a real server 401 — half of this
  gap is closed — but the local-expiry half is still bypassed, and a new regression sits right next to
  the fix.** *Partially addressed by the 2026-08-31 staged rewrite.* `fetch` (`src/client/client.rs:28-54`)
  no longer matches on `ClientError::AuthError`; it now matches `Err(ClientError::HttpError { status,
  body })` directly (`:39-47`) and, when `status == StatusCode::UNAUTHORIZED`, calls
  `auth_manager.refresh_auth().await?` before looping again — so a genuine server-issued 401 (the CDN or
  API actually rejecting the token, item 2 from the previous check) is now retried. Reason 1 from the
  previous check is unchanged, though: the six fetchers' own pre-flight `lock.auth.get_auth().await`
  calls (see the item above) still `return Err(...)` before ever reaching `fetch`, so a token that's
  merely expired *locally* (the common "just launched after an hour" case) still never reaches this
  loop at all — closing this still needs those pre-flight checks folded into (or replaced by) this path.

- [ ] **New regression: any non-401 HTTP error now gets silently retried up to 3 times and its detail
  discarded.** In the same match arm (`src/client/client.rs:39-46`), a non-401 status — 404, 403, 500,
  anything — still falls into the `Err(ClientError::HttpError { status, body })` arm (the `if` only
  gates the refresh call, not the branch itself), so `status`/`body` are bound, thrown away
  (`let _ = body;`, `:40`), and the loop just `continue`s with no backoff. After 3 pointless identical
  attempts, `fetch` returns the generic `ClientError::MaxRetriesReached` (`:53`) instead of the original
  status/body. Before this rewrite, a non-auth `Http` error fell through to `Err(err) => return Err(err)`
  immediately, preserving the status and response body. Concretely: a request for a nonexistent
  depot/product manifest that used to fail fast with a `404` + body now burns two extra network
  round-trips and then reports an uninformative "Max retires reached" with no indication it was ever a
  404. Fix needs the `if status == UNAUTHORIZED { .. } else { return Err(..) }` split reinstated inside
  that arm.

- [x] **Token observer only emits the access token.** *Fixed in `b87d8dd`* —
  `TokenObserver::on_token_refreshed(&self, auth: Auth)` (`src/auth/token_observer.rs:4`) now hands
  over the full `Auth`, so the rotated refresh token and the computed `valid_until` are both
  available to persist. Blocked in practice by the export gap below.

- [x] **`TokenObserver` cannot be implemented outside the crate.** *Fixed in `32e8786`* ("Export Auth
  publicly from the crate"). `src/auth/mod.rs:6` now re-exports `Auth`, and `src/lib.rs:10` re-exports
  it from the crate root, so the trait's `on_token_refreshed(&self, auth: Auth)` signature (unchanged
  since `b87d8dd`) is finally implementable outside the crate, and `Auth::to_string`/`from_string`
  (public inherent methods on the now-public type) are reachable for the `restore_auth` round-trip.
  `GogDl::set_token_observer` (`src/gogdl/gogdl.rs:138`) is the entry point. Residual: `AuthManager`
  and `AuthError` are still crate-private, so a consumer still can't hold or match on either directly
  — see the still-open error-export item below, which is now the closer analogue of this one.

- [ ] **The error detail added in `a962ef7` is invisible to consumers.** `src/lib.rs` exports
  `GogDlError` but none of the error types nested inside it — `AuthError`, `GamesError`,
  `DepotError`, `SecureLinksError`, `DownloadError` all live in private modules. A consumer can match
  `GogDlError::AuthError(_)` (binding with `_` needs no name) but cannot write `AuthError::TokenExpired`,
  so the local-expired-vs-not-yet-authenticated split that commit introduced is reachable only by
  string-matching `Display` output. (`AuthError`'s shape moved again in the 2026-08-31 rewrite — it's
  now `NotAuthenticated`/`TokenExpired`/`AuthDecodeError`/`AuthEncodeError`/`ClientError { inner: String
  }`, renamed from `Unauthorized`/`AuthExpired`; the point stands regardless of the current names.) It
  also can't reach the same condition arriving by the other route: an auth failure surfaces as either
  `GogDlError::AuthError(..)` or `GogDlError::GameError(GamesError::AuthError(..))` (and the
  `Depot`/`SecureLinks` equivalents) depending on which layer produced it, and the nested match can't be
  written at all. Export the error enums, or flatten auth failures to a single top-level variant.

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

- [x] **Twelve `get_auth().await.unwrap()` calls became reachable panics in `998829d`.** *Fixed in
  `9a1f780`* — `grep -rn 'get_auth().await.unwrap()' src/` is now empty. The six *post-refresh* calls
  disappeared along with the old per-fetcher retry blocks that contained them (see the "local expiry"
  and "retry loop" items above — a mixed blessing, since the panic is gone because the code path is
  gone). The six *pre-flight* calls now bind the `Ok` directly instead of re-calling and unwrapping
  (`games/owned_games.rs:28-34`, `games/game_details.rs:30-36`, `games/game_build.rs:36-42`,
  `depot/build_metadata.rs:34-40`, `depot/depot_info.rs:43-49`, `secure_links/secure_links.rs:41-47`).
  Residual, downgraded from unsound to redundant: that pre-flight `get_auth()` result is bound only to
  clone the `AuthManager`/`Auth` for the actual request — `HttpClient::fetch` → `inner_fetch`
  (`src/client/client.rs:61-65`) calls `get_auth()` again internally, so every authed request now pays
  for two `get_auth()` awaits over the same mutex. The call site's own signature changed with this
  latest diff (from `.fetch(Request::GetAuth { url, auth_manager })` to `.fetch(&url,
  Some(auth_manager), decode)`), but the double-`get_auth` shape is unchanged.

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

- [x] **The `inner` mutex is held across the refresh/login network round-trip.** *Fixed in `9a1f780`.*
  `login_with_code` (`src/auth/auth_manager.rs:43-46`) and `refresh_auth` (`:73-76`) each still clone
  `HttpClient` out of the guard and drop the lock before awaiting the request (now via
  `client.fetch_no_retry(...)`, unchanged in shape by the latest diff), and the six call sites clone
  the `AuthManager` out of *their* mutex rather than holding it across the call.

- [ ] **`refresh_lock` serializes refreshes but doesn't collapse them.** `refresh_auth`
  (`src/auth/auth_manager.rs:60-89`) still takes `refresh_lock` (`:62`) and then unconditionally
  performs a refresh with no early-return if another waiter already refreshed — the collapsing defect
  described here is unchanged. Reachability changed again with the current diff: `HttpClient::fetch`'s
  new retry loop does call `auth_manager.refresh_auth()` on an `AuthError` (see the High-severity item
  above), so this is no longer dead code the way it was right after `9a1f780` — but per that same item,
  the only path that reaches it is the narrow TOCTOU race between a fetcher's pre-flight `get_auth()`
  and `inner_fetch`'s second one, so N-concurrent-401s-collapsing-into-one-refresh is still not
  meaningfully exercised in practice. Re-verify once local expiry or a real 401 can actually drive this
  loop (per that item's fix), since that's when concurrent refreshes would actually start happening.

- [ ] **The observer callback runs while the `inner` mutex is held.**
  `observer.on_token_refreshed(...)` is invoked inside the `let mut inner = self.inner.lock().await`
  block (`src/auth/auth_manager.rs:81-87`), unchanged since `b87d8dd`. Same reachability note as
  `refresh_lock` above: nominally callable again via `fetch`'s new retry loop, but only through the
  same narrow race window, so a slow/re-entrant observer stalling auth access is still a theoretical
  rather than exercised risk today.

- [ ] **`is_valid()` has no clock-skew / in-flight margin.**
  `src/auth/auth.rs:26-29` accepts a token that expires one second from now, which will then 401
  mid-request. Subtract a margin (30–60s) so a token about to expire is refreshed up front.

- [ ] **`AuthManager::set_auth` is still unreachable, but for a narrower reason now.**
  `src/auth/auth_manager.rs:105-107` takes an `Auth` (now exported, `32e8786`) and sets it directly,
  bypassing `refresh_auth`/`login_with_code` — but `AuthManager` itself is still not exported from
  `src/lib.rs`, so there is no `&AuthManager` a consumer could call it on. Either export `AuthManager`
  (deliberate, if it's meant as a manual-override entry point) or drop the method.

- [ ] **A registered `TokenObserver` can never be replaced with "none".**
  `set_token_observer` (`src/auth/auth_manager.rs:37-39`) only ever sets `Some`, so a consumer cannot
  detach on logout/teardown; the `Arc<dyn TokenObserver>` lives as long as the `AuthManager`.

## Medium — duplication & consistency

- [ ] **The "fetch → on 401 refresh → retry once" block used to be copy-pasted six times — still is,
  just with an updated call signature.** The six near-identical pre-flight blocks remain
  (`depot/depot_info.rs:43-49`, `depot/build_metadata.rs:34-40`, `games/owned_games.rs:28-34`,
  `games/game_details.rs:30-36`, `games/game_build.rs:36-42`, `secure_links/secure_links.rs:41-47`):
  lock the manager, `get_auth().await` and bail on `Err`, clone the manager, drop the lock. Each is
  still five lines that only ever produce a clone — see the redundant-double-`get_auth` note on the
  fixed panic item above. The current diff mechanically updated each site's call from
  `.fetch(Request::GetAuth { url, auth_manager })`/`Request::AuthDecode` to `.fetch(&url,
  Some(auth_manager), decode)`, but didn't touch the duplicated pre-flight block itself. Centralizing
  this (folding it into `HttpClient::fetch`/`inner_fetch`, or a small helper each fetcher calls) would
  remove the duplication *and* is exactly the change the High-severity retry item above needs anyway —
  one change closing two open items instead of one.

- [x] **`DownloadError` was left out of the `a962ef7` error unification.** *Fixed in `9a1f780`.*
  `DownloadError::Unauthorized` is gone; `src/downloader/error.rs:46-47` now carries `AuthError(#[from]
  AuthError)` like the other four enums, with the 401 mapped to it at `:59`. `c7d39b9` added a
  `DownloadError::Unknown` variant alongside it (mirroring `ClientError::Unknown`), keeping all six
  error enums in lockstep on both fronts; the 2026-08-31 rewrite replaced that with `ClientError(#[from]
  ClientError)` (`src/downloader/error.rs:49-50`) — same lockstep, new shape (see the dead-variant
  finding under Medium below for what that change actually did to the *other* variants on this enum).
  The second half of the original item is still open, restated below as its own Medium finding — the
  downloader's transfer path (`stream_chunk`) still has
  no 401-refresh-retry of its own.

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
  (`src/gogdl/gogdl.rs:86`) returns `SecureLinks`; the exported `ProductBundle` declares
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
  (per `lumen-cli/CLAUDE.md`'s "Auth refresh regression" section). The 2026-08-31 rewrite deleted that
  `impl From<ClientError> for AuthError` entirely (it no longer exists in `src/auth/error.rs`) and
  replaced it with ad hoc `match`es at the two call sites in `auth_manager.rs` (`login_with_code:48-56`,
  `refresh_auth:86-94`): both call `client.fetch_no_retry(&url, None, false)`, match
  `Err(ClientError::AuthError(e)) => return Err(e)` first, and otherwise fall to `Err(e) =>
  AuthError::ClientError { inner: e.to_string() }`. But because `auth_manager` is hardcoded to `None` in
  both calls, `inner_fetch` (`src/client/client.rs:61-65`) can *never* produce `ClientError::AuthError`
  on these paths — that branch only fires when `Some(auth_manager)` is passed and its own `get_auth()`
  fails locally. So every real failure from `AUTH_URL`/`REFRESH_URL` — including a 401 because the
  refresh token is dead, not just refreshable — falls straight to the stringified `ClientError { inner:
  String }` arm. That's `982dc82`'s exact failure mode back in a new shape: a dead-refresh-token 401 and
  a transient network blip are now both just an opaque string a caller can only distinguish by parsing
  `Display` output, with no `AuthExpired`/`Unauthorized`-style variant to match on at all. Fix needs
  either a real status-aware variant on `AuthError` for this case, or at minimum removing the dead
  `Err(ClientError::AuthError(e))` arm so the code doesn't imply a distinction it can't actually make.

- [ ] **`Http`/`UrlParseError`/`NetworkError`/`DecodeError`/`DeflateError` on `DepotError`, `GamesError`,
  `SecureLinksError`, and `DownloadError` are now dead code.** The 2026-08-31 rewrite deleted the four
  hand-written `impl From<ClientError> for XError` blocks that used to translate a `ClientError` into
  one of these per-type variants (e.g. `ClientError::Http{status: 401,..}` → `DepotError::AuthError(...)`,
  everything else → the matching `Depot`-flavored variant), replacing each with a single blanket
  `ClientError(#[from] ClientError)` variant. But every network call in `depot/`, `games/`, and
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

`cargo clippy --lib -- -W clippy::all` reports 32 warnings on the current (2026-08-31) staged tree (31
at `c7d39b9`, 33 at `v0.0.4-restart`/`32e8786`, 30 at `a962ef7`, 34 at `377314c`, 37 at `998829d`). The
+1 from `c7d39b9` is a single new hit inside this diff's own code — "this `if` statement can be
collapsed" at `src/client/client.rs:41`, the nested `if status == UNAUTHORIZED { if let
Some(auth_manager) = ... }` in `fetch`'s retry arm, collapsible to `if status == UNAUTHORIZED && let
Some(auth_manager) = &auth_manager`. Everything else below carries forward unchanged in count from
`c7d39b9`; the previous pass's own two deltas are described as they were found, against the prior
tree:
- **−4** "redundant field names in struct initialization": deleting the `Request` enum removed the
  `Request::GetAuth { url: url, auth_manager: auth_manager }`-style literals `9a1f780` had introduced
  (`depot/depot_info.rs`, `games/owned_games.rs` ×2, `secure_links/secure_links.rs`) in favor of
  positional `fetch(&url, Some(auth_manager), decode)` calls, which can't trigger this lint. One
  pre-existing, unrelated instance remains at `downloader/download_unit.rs:33` (`offset: offset`).
- **+2** "unneeded `return` statement", new at `src/client/client.rs:71` and `:74` — the two
  `return Ok(result);` lines inside `inner_fetch`'s `if let Some(auth_manager) = ...` / `else` arms are
  each the last statement in their block and don't need `return`. (A third, pre-existing instance at
  `downloader.rs:447` is unrelated to this diff.)

Everything else below is unchanged by this diff — the `let`-binding/`path_resolver` patterns included
live in `downloader.rs`/`download_manager.rs`/`product_bundle.rs`, none of which this diff touched:
- [x] Redundant `if let None = ... ` patterns instead of `.is_none()` in five files. *Fixed as a side
  effect of `a962ef7`* — those six pre-flight checks are now `if let Err(err) = ...`, which cleared
  the whole `redundant_pattern_matching` family (the bulk of the 37 → 30 drop).
- [ ] `Auth::is_valid`'s `map_or(false, ...)` should be `is_some_and(...)` (`src/auth/auth.rs:27-28`).
- [ ] `OwnedGames::default()` is a hand-written inherent method that shadows/confuses with
  `std::default::Default` (`src/games/owned_games.rs:14-16`) — implement the trait instead.
- [ ] The "useless use of `format!`" on `format!("https://embed.gog.com/user/data/games")` with no
  interpolation is still there, just moved (`src/games/owned_games.rs:32`).
- [ ] 6 "returning the result of a `let` binding from a block" (`downloader/download_manager.rs:74,89`,
  `downloader/downloader.rs:283,371,459`, `downloader/product_bundle.rs:47`) and 4 "redundant
  redefinition of a binding `path_resolver`" (`downloader/downloader.rs:158,235,294,382`, all the same
  `let path_resolver = path_resolver;` idiom moving it into a closure) — the bulk of the count,
  unchanged since at least `a962ef7`, both confined to the downloader.
- [ ] Module inception (`mod auth`/`mod client`/`mod downloader`/`mod gogdl`/`mod secure_links`, one
  per containing module of the same name).
- [ ] 1 "all variants have the same postfix: `Error`" on `FileSystemError`
  (`downloader/fs/error.rs:6-21`, `enum_variant_names`) — `ClientError` still isn't flagged for this
  despite still having a non-`Error`-suffixed variant among its six (`MaxRetriesReached`, renamed from
  `Unknown` by the 2026-08-31 rewrite; `Http` was renamed to `HttpError` in the same pass, which does
  now match the postfix).
- [ ] A handful of smaller one-offs: an explicit-closure-for-cloning in `build_metadata.rs:48-55`, a
  `len_zero` at `downloader.rs:82`, two `redundant_closure`s (`downloader.rs:115,200`), `io_other_error`
  at `util/hash.rs:66`, a redundant `&` in a `format!` call at `depot_info.rs:51`, and two
  `or_insert_with(Vec::new)` that should be `or_default()` (`downloader/downloadable_product.rs:50`,
  `downloader/product_bundle.rs:42`).

Run `cargo clippy --fix --lib -p gogdl-lib -- -W clippy::all` for a current full list and to
auto-apply most of these — but only against a clean tree; it will also try to "fix" whatever's
mid-edit if run against uncommitted work.

Run `cargo clippy --fix --lib -p gogdl-lib -- -W clippy::all` to auto-apply most of these.
