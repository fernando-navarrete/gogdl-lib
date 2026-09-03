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

A fifth change, staged on top of `3ed4dd0`, is the biggest fix of the whole `9cd8d0a` lineage:
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
`GogDlError::ClientError(ClientError::AuthError(AuthError::TokenExpired))`. That whole sequence landed
as commit `5b7ca2f` ("Handle Auth Failures And Re-Export Errors").

A sixth, uncommitted change staged on top of `5b7ca2f` closes the "two `.parse().unwrap()` calls can
crash" item below: `SecureLinksManager::get_secure_links` (`src/secure_links/links_manager.rs:46-49`)
now matches on `game_id.parse::<i32>()` and returns a new `SecureLinksError::IncorrectGameId(String,
ParseIntError)` variant (`src/secure_links/error.rs:37-38`) instead of unwrapping; and
`DownloadableProduct::get_downloadable_products`'s filter closure
(`src/downloader/downloadable_product.rs:61-70`) does the same, returning `false` (i.e. dropping the
entry) on a parse failure instead of panicking. In the same edit, `ProductBundle::get_download_files`'s
sibling filter (`src/downloader/product_bundle.rs:52-58`) was changed from `.parse().unwrap_or(0)` to
the identical match-and-return-`false` shape — so the "one panics, one silently substitutes 0"
inconsistency this item used to call out is also gone; both now filter the entry out the same way. This
also introduced a leftover debug `println!(product_id)` in the `downloadable_product.rs` filter closure
that doesn't match the fix's own error-handling intent (it's not gated behind any logging facade — none
exists anywhere else in the crate, `grep -rn 'println!\|log::\|tracing::' src/` matches only this one
line) — written up as its own new Low item below.

No automated tests exist yet (`grep -r '#\[test\]' src/` is empty), so treat every fix here as
untested until you add coverage for it.

A seventh pass, same day (2026-08-31), covers two more commits on top of `5b7ca2f`: `31809e9`
("Use AsyncWrite for Streaming Downloads") and `46b6ce3` ("Use Async File Operations for
Downloads"). Together they close the "Blocking syscalls run directly inside async tasks" High-severity
item below in full — see that item for detail. `31809e9` switches the whole chunk-decompress-and-write
path from sync `flate2`/`std::io::Write` to `async-compression`'s `tokio::write::ZlibDecoder` plus a
`tokio::io::BufWriter`, and makes `HttpClient::stream_chunk`'s callback `AsyncFnMut` so the decoder can
be awaited from inside the streaming loop; `46b6ce3` follows up by converting `OffsetWriter` to hold a
`tokio::fs::File` (driven via `AsyncWrite`, not `FileExt::write_at`) and `PathResolver::open_file` to
`tokio::fs::OpenOptions`, removing the crate's last two direct `std::fs`/blocking-syscall call sites
on the download write path.

An eighth pass, same day (2026-08-31), covers two more commits on top of `46b6ce3`: `71d49a8`
("Fail On Incomplete Download Chunks") and `de52483` ("Handle secure link retries in HTTP client").
`71d49a8` closes the "`Downloader::verify` discards its own result" item below in full: `Downloader::verify`
(`src/downloader/downloader.rs:104-131`) now binds `verify_download_units`'s return value instead of
throwing it away as `_missing_units`, and returns the new `DownloadError::ChunkIntegrityCheckFailed(usize)`
when it's non-empty instead of unconditionally returning `Ok(())`. `de52483` gives `HttpClient::stream_chunk`
its own retry loop, closing most of two open items below: it now takes a `&SecureLinksManager`/`game_id`/
`FileType`/`chunk_hash` instead of a pre-built URL, resolves the URL itself from the highest-priority secure
link inside the loop, and on a 401 calls the new `SecureLinksManager::invalidate_secure_links` before
re-fetching and retrying (up to 3 attempts) — the same shape `fetch` already used for JSON requests, but a
parallel, hand-rolled copy rather than a shared funnel. It also gained its own `AuthError::TokenExpired` arm
that calls `refresh_auth`, mirroring `fetch`'s. This closes the "bypasses the `fetch`/`inner_fetch` funnel
entirely" item below (via a parallel implementation, not by actually going through the funnel — see that
item's update for the caveat) and the "no refresh path" half of the "secure links cached forever" item
below — but note both fixes are scoped to 401/auth-shaped failures only; see the updated "no retry/backoff
for transient network failures" item for what's still not covered, and the new concurrency item this pass
added for a thundering-herd gap the fix introduces under concurrent chunk downloads.

A ninth pass, same day (2026-08-31), covers `e4a8560` ("Retry transient network errors in chunk
streaming"), the very item the eighth pass said was still open. `HttpClient::stream_chunk`
(`src/client/client.rs:71-144`, grew from `71-135`) now retries its two remaining unhandled failure
modes too: the non-401 branch of the `HttpError` arm (`:122-127`, a `5xx` or other CDN error) now
`continue`s while `attempts < 3` instead of returning immediately, and a new
`Err(ClientError::NetworkError(err))` arm (`:132-137`, a dropped connection or timeout inside
`stream_chunk_inner`'s `bytes_stream()`) does the same. Both give up and propagate
(`ClientError::HttpError`/`ClientError::NetworkError` respectively) only once `attempts` has reached 3,
same as the pre-existing 401/`TokenExpired` arms. This closes the "no retry/backoff for transient
network failures during download" item below for the chunk-transfer path specifically — see that item's
update for what's still uncovered (no backoff/delay between attempts, and `fetch`'s separate retry loop
for JSON/API calls still doesn't handle `NetworkError` either) — and removes the stale caveat from the
"bypassed the funnel" item below. `cargo clippy` still reports 35 warnings after this change (unchanged
from the eighth pass): the two pre-existing `client.rs` warnings inside `stream_chunk` shifted down by 9
lines to `:126`/`:130`, and the new code (`if attempts < 3 { continue; }` and the plain `NetworkError`
arm) introduced none of its own.

A tenth pass, same day (2026-08-31), covers `2100296` ("Check Disk Space Before Downloading"). This
closes the "SHA-256 support is unused dead code" item below, but by deletion rather than by finally using
the dead code: `DepotFile.sha256` (`src/depot/depot_info.rs`) and the whole `ChecksumAlgorithm` enum/
`Sha256` variant (`src/downloader/util/hash.rs`) are gone outright, `compute_chunk_checksum` now always
hashes MD5, and `sha2`/`cpufeatures` dropped out of `Cargo.lock`. The commit's main addition is a
proactive free-space check: `Downloader::download` (`src/downloader/downloader.rs:66-78`) now sums
`depot_file.size()` (a new method, `:35-41`, summing chunk sizes) over the files the size-verification
step already found missing, calls the new `PathResolver::get_free_space` (`src/downloader/fs/
path_resolver.rs:39-55`, finds the `sysinfo::Disks` entry whose mount point `canonical_base` starts with
and reads `available_space()`), and fails the whole job up front with `DownloadError::NotEnoughFreeSpace`
if the required total exceeds it, or `DownloadError::CouldNotResolveFreeSpace` if no matching disk is
found at all. This also resolves the "one failed file allocation aborts the whole download" item below —
not by changing that behavior, but because that behavior turns out to be correct on inspection: there is
no point continuing a download that is already known to be short on disk space, whatever the specific
cause of an allocation failure turns out to be, so failing the whole job fast is the right call, not a
bug. See both items' entries below for the update. One new gap this pass introduces: `get_free_space`'s
own error is discarded at its one call site (`Err(_) => return Err(DownloadError::CouldNotResolveFreeSpace)`,
`:74`) — the new `FileSystemError::NoDiskMatchingPath(PathBuf)` variant it can return, which names exactly
which `canonical_base` had no matching disk, never reaches the caller. This is the same "error detail
discarded" shape this doc has flagged repeatedly elsewhere (see the "error detail invisible to consumers"
and "dead refresh token vs. routine expiry" items), just on a path that has no other coverage yet — noted
as its own new Low item below. `cargo clippy` moves 35→36: **+1** `manual_map` on the new
`DepotFile::size` (`depot_info.rs:36-40`, the `if let Some(chunks) = &self.chunks { Some(...) } else {
None }` shape should be `self.chunks.as_ref().map(...)`), **+1** `needless_return` on the new
`get_free_space`'s `return Err(FileSystemError::NoDiskMatchingPath(...))` (`path_resolver.rs:49-51`,
sitting inside a `match` arm where a bare expression would do), and **−1** `enum_variant_names` on
`FileSystemError` — the Low-severity item below noting `FileSystemError` was flagged for "all variants
have the same postfix: Error" no longer applies now that the new `NoDiskMatchingPath` variant breaks that
uniformity, so the lint stopped firing on it (not a fix, just a side effect of adding a differently-named
variant).

An eleventh pass, 2026-09-03, covers `a11273a` ("Add game repair workflow and chunk metadata mapping"),
the sole commit between `v0.0.9-restart` (`8e8a392`) and the newly-pushed `v0.0.10-restart`. It also
belatedly covers the three commits between the tenth pass's `2100296` and `v0.0.9-restart` that were
marked fixed inline but never got a narrative entry (`d679048`, `51b7a53`, `8e8a392` — all three items
are ticked in the auth section below and were re-verified against the current tree this pass).

`a11273a` adds a repair workflow and re-shapes how download units are built:
- **New public API:** `GogDl::repair_game` → `DownloadManager::repair_game` → `Downloader::repair`
  (`src/downloader/downloader.rs:39-138`). `repair` runs four stages — file-size verification, disk-space
  check + allocation, **per-chunk MD5 verification**, then download of only the units that failed
  verification — where `download` runs three and downloads every unit unconditionally. Repair is
  therefore the crate's only incremental/resume-shaped path (see the new consistency item below).
- **New progress variant:** `DownloadStageEvent::VerificationStage(VerificationEvent)`, emitted only by
  `repair`'s stage 3. This is a breaking change for consumers that match the enum exhaustively — see the
  new `lumen-cli` item below.
- **Chunk-metadata mapping moved:** `DownloadUnit::from_depot_file(DepotFile)` is replaced by
  `DepotFile::to_download_units(&self, product_id: String)` (`src/depot/depot_info.rs:45-68`) plus
  `DownloadUnit::from_product_bundles(&[ProductBundle])` (`src/downloader/download_unit.rs:22-32`), and
  `DownloadUnit` gained a `product_id: String` field, so the `(String, DownloadUnit)` tuple
  `download_files` used to carry is gone. `download_files` now takes an already-built
  `Vec<DownloadUnit>` instead of `&[ProductBundle]`, which is what lets `repair` hand it a filtered
  subset. Clean refactor; no behavior change on its own.
- **Silently removed:** `download_files`'s secure-links pre-fetch loop (the `stream::iter(bundles)
  .map(..).buffer_unordered(self.threads)` block that warmed `SecureLinksManager`'s cache before any
  chunk started). Nothing replaced it — see the updated thundering-herd item below, which this turns
  from an expiry-only corner case into something that fires at the start of every download and repair.

Two findings this pass are not about `a11273a` at all — they are pre-existing on `restart` and simply
went unnoticed until the repair path made `download_files` load-bearing for a second workflow. Both are
regressions *relative to mainline `master`*, where the same problems were found and fixed for `v0.1.1`:
the retry-into-a-dirty-writer bug (High, below) and `DownloadEvent::Progress` being emitted per network
read rather than per buffer flush (High, below — the latter is the memory leak the workspace
`CLAUDE.md` documents as the reason `v0.1.1` exists). The first of these also partly invalidates a
checkbox the ninth pass ticked, which is re-opened below. `cargo build --lib` is clean (no rustc
warnings); `cargo clippy --lib -- -W clippy::all` moves 37→39 (see the clippy section for the
breakdown, including the 36→37 step the unlogged `51b7a53` introduced).

**`a11273a` closes no previously-open item in this doc.** One older item did get ticked this pass, but
for a fix that landed back on 2026-08-31: the leftover debug `println!` was removed in `1d98eb7`
("Remove Debug Print From Product Filtering", first tagged in `v0.0.7-restart`) and the checkbox was
simply never updated — `grep -rn 'println!\|eprintln!\|log::\|tracing::' src/` is empty crate-wide now.
Still true and re-verified this pass: no automated tests exist anywhere
(`grep -rn '#\[test\]\|#\[tokio::test\]' src/` returns nothing), so every fix in this doc — including
the four the repair work now depends on — remains untested.

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

- [x] **Two `.parse().unwrap()` calls could crash the process on bad input, and a sibling file handled
  the identical failure differently.** *Fixed in the uncommitted change on top of `5b7ca2f`.*
  - `src/secure_links/links_manager.rs:46-49` — `game_id.parse::<i32>()` (where `game_id: &str` is a
    public API parameter that ultimately comes from the Flutter/Dart side across FFI) is now matched,
    returning the new `SecureLinksError::IncorrectGameId(String, ParseIntError)` variant
    (`src/secure_links/error.rs:37-38`) instead of unwrapping. A malformed caller-supplied id now
    returns an error instead of panicking the whole process.
  - `src/downloader/downloadable_product.rs:61-70` — the filter closure now matches
    `product_id.parse::<i32>()` and returns `false` (dropping the entry) on `Err` instead of unwrapping.
  - `src/downloader/product_bundle.rs:52-58` — changed from `.parse().unwrap_or(0)` to the same
    match-and-`false` shape as the item above, so the previous inconsistency (one path panicking, the
    other silently substituting `0`) is gone too — both now filter the unparseable entry out the same
    way.
  - Caveat: the `downloadable_product.rs` filter closure also picked up a leftover debug
    `println!("{}", product_id)` in this same edit — see the new Low item below.

- [x] **Twelve `get_auth().await.unwrap()` calls became reachable panics in `998829d`.** *Fixed in
  `9a1f780`, and the residual double-`get_auth()` cost noted in the previous pass is now also gone.* —
  `grep -rn 'get_auth().await.unwrap()' src/` is still empty, and this pass deleted the six fetchers'
  pre-flight `get_auth()` blocks entirely (see the duplication item below) rather than just binding their
  `Ok` — so there's no longer a redundant pre-flight call being paid for on top of the one
  `inner_fetch` makes internally (`src/client/client.rs:87-90`). Every authed request now does exactly
  one `get_auth()` await over the mutex, down from two.

## High — downloader reliability

- [ ] **No retry/backoff for transient network failures during download.** *Re-opened by the eleventh
  pass. The retry loop exists, but the arm that handles the mid-transfer case cannot actually succeed —
  see the new "retry resumes into a dirty writer" item directly below; only the pre-body failure shapes
  (a CDN 401 and a non-401 status, both detected before any body byte is delivered) genuinely retry
  today. The rest of this entry describes what the code intends, which is still accurate as intent.*
  *Fixed for the chunk-transfer
  path in `e4a8560` ("Retry transient network errors in chunk streaming"), on top of `de52483`'s
  auth-shaped retries.* `HttpClient::stream_chunk` (`src/client/client.rs:71-144`) now retries all four
  failure shapes it can hit, not just the two auth-shaped ones: a CDN 401 (invalidate + re-fetch secure
  links), `AuthError::TokenExpired` (refresh), a non-401 `HttpError` such as a CDN `5xx` (`:122-127`,
  `if attempts < 3 { continue; }`), and `ClientError::NetworkError` from a dropped connection/timeout
  inside `stream_chunk_inner`'s `bytes_stream()` (`:132-137`, same pattern). A single timeout, connection
  reset, or 5xx on any one chunk no longer aborts the entire multi-gigabyte download outright — it
  retries up to 3 attempts before `download_files`'s
  `results.into_iter().collect::<Result<(), DownloadError>>()?` (`src/downloader/downloader.rs:245-308`,
  moved by `a11273a`) ever sees the error. Two things are still not covered: there's no backoff/delay
  between attempts (each retry is an immediate `continue`, so a sustained outage burns through all 3
  attempts near-instantly rather than spacing them out), and this fix is scoped to `stream_chunk` only —
  `fetch` (`src/client/client.rs:40-70`), the funnel for every JSON/API request, still has no arm for
  `ClientError::NetworkError` and falls straight to `Err(err) => return Err(err)` at `:64-66` on one.

- [ ] **`stream_chunk`'s retry resumes into a dirty writer, so the one failure shape its `NetworkError`
  arm exists for can never actually recover.** *New in the eleventh pass; the bug predates `a11273a` (it
  arrived with `e4a8560`) but is only now written up.* `HttpClient::stream_chunk`
  (`src/client/client.rs:71-144`) retries by looping back and calling `stream_chunk_inner(&url, &mut f)`
  again with **the same `f`** — and `f` is the closure built in `download_files`
  (`src/downloader/downloader.rs:272-277`) that writes into a single
  `ZlibDecoder<HashingWriter<BufWriter<OffsetWriter>>>` stack. Nothing in that stack is reset between
  attempts: `OffsetWriter`'s `pos`/`remaining` (`src/downloader/util/offset_writer.rs:58-59`) only ever
  advance, the `ZlibDecoder` keeps its stream state, and `HashingWriter` keeps accumulating MD5. So a
  retry restarts the HTTP GET at byte 0 and appends a fresh zlib stream onto a half-consumed one at the
  wrong file offset. Every outcome is a failure: a zlib decode error, an
  `OffsetWriter` "chunk decompressed past its declared size" error, or an MD5 mismatch at
  `downloader.rs:286`. The two arms that *do* recover (`HttpError` 401 → invalidate secure links;
  non-401 `HttpError`) are safe purely by accident — `stream_chunk_inner` checks
  `response.status()` (`:211`) before touching `f`, so no bytes have been written when they fire. The
  `ClientError::NetworkError` arm (`:132-137`) is the broken one, and `NetworkError` is exactly the
  mid-body case: `stream.next()` yielding an `Err` at `:225`, after some bytes have already gone through
  `f`. (`NetworkError` also covers a pre-body connect/`send()` failure — `#[from] reqwest::Error`,
  `src/client/error.rs:11` — so the arm is correct for that half and broken for the other half, with no
  way for the caller to tell them apart.) Mainline `master` solved this in `stream_unit_to_file`
  (`src/downloader/stream.rs`) by re-seeking the file to the unit's `offset` at the top of *every*
  attempt and carrying a `counted` high-water-mark across attempts; the `restart` branch needs the
  equivalent — rebuild the decoder/writer stack (or re-seek and reset it) inside the retry loop rather
  than outside it.

- [ ] **`DownloadEvent::Progress` is emitted once per network read — the exact event-rate problem
  mainline `v0.1.1` was cut to fix, now on two code paths.** *New in the eleventh pass; predates
  `a11273a`, but `repair` makes `download_files` the transfer path for a second public workflow.*
  `download_files`'s callback sends `DownloadEvent::Progress(chunk.len())` for every `Bytes` the
  `bytes_stream()` yields (`src/downloader/downloader.rs:274`), into an unbounded channel that a
  forwarding task re-sends into another unbounded channel (`:210-214` for `download`, `:129-133` for
  `repair`). With `self.threads` chunks in flight on a fast connection that is thousands of sends per
  second, bounded by read syscall size rather than by throughput. The workspace `CLAUDE.md` documents
  this same shape as the repair-download memory leak that `master`'s `v0.1.1` exists to fix: there,
  `gogdl_flutter`'s drain loops flooded an unbounded `StreamSink` faster than Dart could drain it, and
  the fix had two halves — coalescing in the bridge *and* cutting the rate at the source, by moving
  `Progress` to the write-buffer flush boundary (`master:src/downloader/stream.rs:45-70`, which spells
  out the reasoning). The `restart` branch has neither half. Emit `Progress` at the `BufWriter`
  flush boundary (or on a fixed byte/time threshold) before any Flutter consumer is wired up to
  `repair_game`.

- [x] **One failed file allocation aborts the whole download.** *Resolved as intended behavior, not a
  bug — confirmed with the person driving this rebuild.* `Downloader::download`
  (`src/downloader/downloader.rs:195-200`, shifted down by `a11273a`'s new `repair` above it; `repair`
  carries an identical copy at `:96-101`) still fails the
  entire job the moment `allocate_missing_files` reports any failure, and that is the correct call: if
  the disk cannot hold the files being allocated, there is no point continuing the download at all,
  whatever the specific underlying cause of the allocation failure turns out to be. `2100296` ("Check
  Disk Space Before Downloading") reinforces this rather than replacing it — a new proactive check right
  before the allocation step (`:166-178` in `download`, copied verbatim to `:67-79` in `repair`) sums
  the size of every file the size-verification step found
  missing, compares it against `PathResolver::get_free_space()`'s reading of the target disk's
  `available_space()`, and fails fast with `DownloadError::NotEnoughFreeSpace` (or
  `DownloadError::CouldNotResolveFreeSpace` if no disk matches the target path) before allocation even
  starts, so the common "not enough disk space" case now gets a specific, actionable error instead of the
  generic `DownloadError::FileAllocationError` from the abort this item used to flag.

- [x] **`Downloader::verify` discards its own result.** *Fixed in `71d49a8` ("Fail On Incomplete
  Download Chunks").* `Downloader::verify` (`src/downloader/downloader.rs:220-243`, shifted by
  `a11273a`) now binds the
  `verify_download_units` result as `missing_units` instead of `_missing_units`, and returns
  `Err(DownloadError::ChunkIntegrityCheckFailed(missing_units_count))` (new variant,
  `src/downloader/error.rs:48-49`) when `missing_units.len() > 0` instead of unconditionally returning
  `Ok(())`. A caller that isn't listening on the `tx` event channel now learns verification failed via
  the `Result` itself too.

- [x] **Blocking syscalls run directly inside async tasks.** *Fixed across `31809e9` ("Use AsyncWrite
  for Streaming Downloads") and `46b6ce3` ("Use Async File Operations for Downloads").*
  `PathResolver::open_file` (`src/downloader/fs/path_resolver.rs:112-123`) now opens via
  `tokio::fs::OpenOptions::new().write(write).open(&path).await` instead of the synchronous
  `std::fs::OpenOptions`, matching `allocate_file` right above it. The chunk write path no longer calls
  a blocking positioned write at all: `OffsetWriter` (`src/downloader/util/offset_writer.rs`) now holds
  a `tokio::fs::File` and implements `AsyncWrite` by polling that file directly (`poll_write` at
  `:29-58`), replacing the old `FileExt::write_at` call; it's wrapped in a `tokio::io::BufWriter` and
  driven from `stream_chunk`'s callback, which `31809e9` changed from `FnMut` to `AsyncFnMut` so
  `decoder.write_all(&chunk).await` (`src/downloader/downloader.rs:196-198`) can actually yield instead
  of blocking the worker thread. Neither fix uses `spawn_blocking` — they replace the blocking calls
  with real async I/O instead, which is a cleaner fix than wrapping them. `OffsetWriter::new` itself now
  takes a `tokio::fs::File` directly rather than converting one via `from_std`. `grep -rn 'std::fs::\|
  write_at' src/downloader/` is empty — no blocking filesystem call remains anywhere on the download
  write path.

- [x] **Secure links are cached forever with no expiry handling.** *The "no refresh path" half fixed
  in `de52483` ("Handle secure link retries in HTTP client"); the proactive half is still open, restated
  below.* `SecureLinksManager::links_cache` (`src/secure_links/links_manager.rs:19,32-63`) still never
  evicts an entry on its own, and the fetched `CdnUrlParams`'s `expires_at`/`ttl` fields
  (`src/secure_links/secure_links.rs:11,13`) are still parsed but never consulted proactively — but a
  link that expires mid-download is no longer a dead end. `HttpClient::stream_chunk`
  (`src/client/client.rs:71-144`) now reacts to a 401 from the CDN itself by calling the new
  `SecureLinksManager::invalidate_secure_links` (`:64-67`, removes the cache entry) and re-fetching
  before retrying, so the crate finally retries on a CDN-side 401, not just a GOG-API one. What's still
  missing is purely proactive: nothing checks `expires_at`/`ttl` ahead of a request the way a `is_valid()`
  margin would for auth tokens (see that Medium item below) — every chunk request still has to hit a 401
  once before the stale link gets replaced, and see the new concurrency item below for what happens when
  several chunks hit that 401 at once. `a11273a` made the surrounding situation worse in one respect:
  it deleted `download_files`'s secure-links pre-fetch loop, which used to populate the cache for every
  bundle before the first chunk started, so the cache is now cold at the start of every download and
  repair (see the updated concurrency item below).

- [x] **Only per-chunk MD5 is verified; SHA-256 support is unused dead code.** *Resolved by deletion in
  `2100296` ("Check Disk Space Before Downloading"), not by finally using the dead code.* `DepotFile.sha256`
  is gone from `src/depot/depot_info.rs`, and `ChecksumAlgorithm`/`ChecksumAlgorithm::Sha256` are gone
  entirely from `src/downloader/util/hash.rs` — `compute_chunk_checksum` no longer takes an algorithm
  parameter and always hashes MD5, and the `sha2`/`cpufeatures` dependencies dropped out of `Cargo.toml`/
  `Cargo.lock` with it. Whether GOG's whole-file SHA-256 manifest entries turn out to matter later is now
  a fresh feature decision rather than an unfinished one sitting half-wired in the tree.

## Medium — auth concurrency & lifetime

- [x] **The `inner` mutex is held across the refresh/login network round-trip.** *Fixed in `9a1f780`,
  unaffected by this pass beyond the file move.* `login_with_code` (`src/client/auth/auth_manager.rs:
  50-64`) and `refresh_auth` (`:73-96`) each take their tokens/refresh-token out of the guard, drop the
  lock, and only then await the request via `client.fetch_no_retry(...)`.

- [x] **`refresh_lock` serializes refreshes but doesn't collapse them — and this is now the common-case
  path, not a corner case.** *Fixed:* `refresh_auth` (`src/client/auth/auth_manager.rs:70-108`) now
  snapshots the current `access_token` *before* queueing on `refresh_lock`. Once the lock is acquired it
  re-reads the stored token and compares it against the pre-lock snapshot; if they differ, another
  waiter already completed a refresh while this call was queued, so it returns `Ok(())` immediately
  instead of performing a second network round-trip. Under a burst of N concurrent requests hitting
  expiry at once, only the first waiter actually calls `REFRESH_URL` and fires
  `on_token_refreshed`; the rest short-circuit and let the caller retry with the token the first waiter
  installed. Token-identity comparison was chosen over re-checking `is_valid()` after the lock because
  the latter would also swallow a genuine 401-driven refresh request for a token that is still locally
  valid but was rejected/revoked server-side — that case still needs to reach the network. The one
  remaining non-collapsed case (a 401 arriving after an unrelated refresh already landed) still performs
  exactly one redundant refresh, which is rare and bounded.

- [x] **The observer callback runs while the `inner` mutex is held.** *Fixed in `51b7a53` ("Avoid
  Holding Lock During Token Refresh Callback").* `refresh_auth` (`src/client/auth/auth_manager.rs:
  110-121`) now clones the observer out of the guard, `drop(inner)`s it, and only then calls
  `observer.on_token_refreshed(...)` outside the lock. A slow or re-entrant `TokenObserver` can no
  longer stall other in-flight authed requests waiting on `inner`.

- [x] **`is_valid()` has no clock-skew / in-flight margin.** *Fixed in `d679048` ("Add token observer
  removal and expiry buffer").* `src/client/auth/auth.rs:32-35` now subtracts a 60s buffer
  (`t > chrono::Utc::now().timestamp() - 60`), so a token within 60s of expiry is treated as already
  invalid and refreshed up front instead of being handed out and 401ing mid-request.

- [x] **`AuthManager::set_auth` is still unreachable, but for a narrower reason now.** *Resolved by
  removal.* The method no longer exists — `grep -n 'fn set_auth' src/` is empty, and the current
  `src/client/auth/auth_manager.rs` only exposes `new`/`get_login_url`/`set_token_observer`/
  `login_with_code`/`restore_from_string`/`refresh_auth`/`get_auth`. Whether that was a deliberate
  cleanup or a side effect of the rewrite, the dead/unreachable method this item flagged is gone either
  way.

- [x] **A registered `TokenObserver` can never be replaced with "none".** *Fixed in `d679048` ("Add
  token observer removal and expiry buffer").* `AuthManager::remove_token_observer`
  (`src/client/auth/auth_manager.rs:40-42`) sets `token_observer` back to `None`, and the method is
  wired all the way out through `HttpClient::remove_token_observer` (`src/client/client.rs:159-161`) and
  `GogDl::remove_token_observer` (`src/gogdl/gogdl.rs:129-131`), so a consumer can now detach on
  logout/teardown.

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

- [x] **`HttpClient::stream_chunk` bypassed the `fetch`/`inner_fetch` funnel entirely.** *Given its own
  parallel retry loop across `de52483` ("Handle secure link retries in HTTP client") and `e4a8560`
  ("Retry transient network errors in chunk streaming"), rather than being rewritten to go through the
  funnel.* `stream_chunk` (`src/client/client.rs:71-144`) now retries up to 3 attempts and handles a
  CDN-side 401 (invalidate + re-fetch secure links, see the fixed item above), `AuthError::TokenExpired`
  (calls `refresh_auth`, same as `fetch`'s arm at `:61-63`), a non-401 `HttpError`, and
  `ClientError::NetworkError` — so the method that moves the actual multi-gigabyte payload is no longer a
  dead end for any of its failure modes (see the "no retry/backoff for transient network failures" item
  above for the one remaining gap, a lack of backoff between attempts). It still isn't unified with
  `fetch`/`inner_fetch`: the retry `match` is hand-copied rather than shared (both now independently
  contain a `HttpError{status,body}` arm and a `TokenExpired` arm — a future change to one's retry logic
  won't propagate to the other without someone remembering to update both), and `fetch` itself still has
  no `NetworkError` arm, so the two loops have now drifted further apart in what they cover, not closer.
  Two corrections from the eleventh pass, both consequences of that hand-copying: (a) `stream_chunk`'s
  `Err(ClientError::AuthError(AuthError::TokenExpired))` arm (`src/client/client.rs:129-131`) is dead
  code — it was copied from `fetch`, but `stream_chunk_inner` never calls `get_auth()` (CDN URLs are
  pre-signed and `:207` builds the request with no bearer token), so the only error variants it can
  return are `UrlParseError`/`NetworkError`/`HttpError`/`ChunkStreamCallbackError`. An auth failure
  during the *secure-links* fetch is already converted to `ClientError::SecureLinksError { inner: String }`
  and returned without retrying at `:86-91`. (b) The retry itself doesn't work for the mid-transfer case
  — see the "retry resumes into a dirty writer" item in the High section.

- [ ] **`Downloader::repair` is a near-verbatim copy of `Downloader::download`.** *New in `a11273a`.*
  `repair` (`src/downloader/downloader.rs:39-138`) and `download` (`:139-219`) are the same function
  apart from one inserted stage. Lines `45-101` of `repair` and `145-200` of `download` — path-resolver
  construction, the `depot_files` flat_map, the whole file-size-verification stage with its
  channel/`tokio::join!`/forwarding-task boilerplate, the free-space computation and its two error
  returns, the whole allocation stage with the same boilerplate again, and the `FileAllocationError`
  abort — are identical modulo two extra `drop()`s and the stage-forwarding variable names
  (`tx_stage3`/`tx_stage4` vs `tx_stage3`). The only real difference is that `repair` inserts a
  `verify_download_units` stage (`:103-121`) and passes its `missing_units` to `download_files`, where
  `download` passes `DownloadUnit::from_product_bundles(&bundles)` wholesale. Every fix this doc lists
  against `download`'s pipeline now has to be applied twice, and the free-space error-discard item in
  the Low section already exists in two places because of it. `download` is expressible as `repair` with
  the verification stage skipped (or better: as a shared private helper taking a "which units to
  download" closure).

- [ ] **`download` re-downloads every chunk regardless of what's already correct on disk, so `repair` is
  now the crate's only resume path — and nothing says so.** *Pre-existing, made visible by `a11273a`.*
  `Downloader::download` runs a file-size verification stage and computes `missing_files`, then ignores
  that result when building the transfer list: `DownloadUnit::from_product_bundles(&bundles)`
  (`src/downloader/downloader.rs:205`) enumerates every chunk of every file in every bundle, including
  files that just verified as complete and correctly sized. An interrupted download restarted through
  `download_game` therefore re-transfers the whole game from byte 0, while `repair_game` transfers only
  what fails MD5. Given pause/resume was deliberately removed chain-wide (see the workspace
  `CLAUDE.md`), `repair_game` is the de-facto resume entry point, and a consumer has no way to know that
  from the API surface — the two methods have identical signatures and neither has a doc comment.
  Either make `download` skip already-verified units, or document that resuming means calling
  `repair_game`.

- [ ] **`DownloadStageEvent::VerificationStage` is a source-breaking addition for existing consumers.**
  *New in `a11273a`.* The variant (`src/downloader/progress_reporting/download_stage_event.rs:9`) is
  emitted only by `repair`'s stage 3, but it widens a `pub` enum that consumers match exhaustively.
  `lumen-cli` (`/home/fernando/repo/lumen-project/lumen-cli`, sibling repo, currently pinned at
  `tag = "v0.0.9-restart"` in its `Cargo.toml:12`) has an exhaustive four-arm `match ev` with no `_`
  arm in `apply_download_stage_event` (`src/middleware/downloads.rs:555-607`), so bumping its pin to
  `v0.0.10-restart` is a hard `E0004` compile break — on top of the `E0599: no method named
  'get_secure_links'` break the export item below already flags for the same pin bump. Its
  `DownloadStage` enum has no verification state to map the new variant onto either, so this needs a
  code change there, not just a new arm. Same applies to `gogdl_flutter`'s `restart` branch whenever
  `repair_game` gets bridged.

- [ ] **`repair` checksums the chunks of files it has just allocated.** *New in `a11273a`.* Stage 2
  allocates every file that failed size verification (`set_len` on a fresh or truncated file,
  `src/downloader/fs/path_resolver.rs:76`), and stage 3 then MD5s **every** unit of **every** file
  (`src/downloader/downloader.rs:103-115`) — including those just-allocated, all-zero ranges, whose
  chunks cannot possibly match. On a repair where a large file is missing outright, that's a full-size
  read plus MD5 of zeroes purely to conclude what stage 1 already knew. Filtering the units belonging to
  `missing_files` out of the verification pass (and adding them straight to the download list) skips
  that entirely.

- [ ] **Secure-link fetches aren't collapsed across concurrent chunk downloads — and since `a11273a`
  deleted the pre-fetch that used to hide it, this fires at the start of every download and repair, not
  just on expiry.** `SecureLinksManager::get_secure_links`/`invalidate_secure_links`
  (`src/secure_links/links_manager.rs:32-67`) have no in-flight-request dedup, unlike the note this doc
  already has on `refresh_lock` not collapsing concurrent auth refreshes below. `Downloader::download_files`
  runs up to `self.threads` chunk downloads concurrently via `buffer_unordered`
  (`src/downloader/downloader.rs:254,302`), each calling `HttpClient::stream_chunk` independently, and
  `stream_chunk` calls `get_secure_links` per chunk (`src/client/client.rs:84`).
  - *Cold-start (new with `a11273a`):* `download_files` used to warm the cache first — a
    `stream::iter(bundles).map(|bundle| get_secure_links(product_id)).buffer_unordered(self.threads)`
    block that `a11273a` removed along with the `(String, DownloadUnit)` tupling it sat next to. Nothing
    replaced it, so the first `self.threads` chunk tasks now all miss the empty `links_cache` at once and
    each issue its own `SecureLinks::get_secure_links` round-trip. The cache-check and the fetch are not
    under one lock (`links_manager.rs:39-60`), so the misses aren't collapsed by the mutex either.
    This is per product bundle, on every download and every repair.
  - *Expiry (as before):* if a game's secure link expires while several chunks are in flight, every one
    of them can hit the CDN's 401 at roughly the same time; each independently calls
    `invalidate_secure_links` (removing the same already-removed cache entry is harmless) and then
    independently re-fetches, instead of one task refreshing and the rest reusing its result.

  In both cases a single event can trigger up to `self.threads` redundant secure-link fetches instead of
  one. Restoring the pre-fetch would paper over the cold-start half; an in-flight dedup (a per-`game_id`
  `OnceCell`/shared future, the shape `PathResolver::dir_cache` already uses at
  `src/downloader/fs/path_resolver.rs:87-99`) fixes both.

- [ ] **Types a consumer must be able to name are not exported (`DepotFile`, `Chunk`, and — since
  `a11273a` — `DownloadUnit`).** The exported
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
  `SecureLinks` is no longer a type a consumer needs to name at all. `a11273a` widened the leak slightly
  rather than narrowing it: `DepotFile::to_download_units` (`src/depot/depot_info.rs:45`) is a new `pub`
  method on the unexported `DepotFile`, returning `Vec<DownloadUnit>` — and `DownloadUnit` is a third
  type that isn't re-exported from `src/lib.rs`. It's only reachable through `ProductBundle.product_files`,
  so nothing *newly* breaks, but it does mean the public field now leaks three unnameable types instead
  of two. `lumen-cli`
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
  `a11273a` sharpens this: it adds a public `GogDl::repair_game` and a new `DownloadStageEvent` variant
  with no doc comment anywhere, and the download-vs-repair semantics (which one resumes, which one
  re-transfers everything, which stages each emits) are only discoverable by reading `downloader.rs`.
  This crate now has two near-identical public entry points whose difference is undocumented — see the
  consistency items above.

## Low — style / clippy

- [x] **Leftover debug `println!` in a filter closure.** *Fixed in `1d98eb7` ("Remove Debug Print From
  Product Filtering", 2026-08-31, first tagged in `v0.0.7-restart`) — the fix landed four tags ago but
  this item was never ticked; caught on the eleventh pass.* The line in
  `src/downloader/downloadable_product.rs` that printed every candidate `product_id` on each call to
  `get_downloadable_products` is gone, and `grep -rn 'println!\|eprintln!\|log::\|tracing::' src/` is now
  empty crate-wide. It was never flagged by `cargo clippy --all -- -W clippy::all` (`print_stdout` is an
  allow-by-default restriction lint, not part of the `clippy::all` group this doc's warning count
  tracks), so the fix shows up nowhere in the counts above. The crate still has no logging facade at
  all — if one is ever adopted, this is the kind of call site that would want it.

- [ ] **The free-space check discards its own error detail — now in two places.** `Downloader::download`
  (`src/downloader/downloader.rs:172-175`) and, since `a11273a`, the copy of the same block in
  `Downloader::repair` (`:73-76`) each match `path_resolver.get_free_space()`'s `Err(_)` and always
  return the generic `DownloadError::CouldNotResolveFreeSpace`, throwing away the
  `FileSystemError::NoDiskMatchingPath(PathBuf)` that names exactly which resolved base path had no
  matching disk (`src/downloader/fs/path_resolver.rs:39-55`). Same "detail thrown away at the call site"
  shape this doc has flagged elsewhere (see the error-detail items in the auth/client sections) — carry
  the path (or the whole `FileSystemError`) into `DownloadError::CouldNotResolveFreeSpace` instead of a
  unit variant.

**Current count: 39 warnings at `a11273a` / `v0.0.10-restart`** — see the eleventh-pass update at the end
of this section for the breakdown. The paragraph below and the bullets that follow it were written at the
sixth pass and are kept for the history; where a location has moved since, the bullet says so.

`cargo clippy --lib -- -W clippy::all` reports 34 warnings on the then-current (2026-08-31, sixth pass) staged
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
- [ ] 7 "returning the result of a `let` binding from a block" (`downloader/download_manager.rs:70,85,100`
  — the third added by `a11273a`'s `repair_game`, `downloader/downloader.rs:366,454,539`,
  `downloader/product_bundle.rs:47`) and 4 "redundant redefinition of a binding `path_resolver`"
  (`downloader/downloader.rs:256,318,377,465`, all the same `let path_resolver = path_resolver;` idiom
  moving it into a closure) — the bulk of the count, unchanged in shape since at least `a962ef7`, both
  confined to the downloader. Note `DownloadUnit::from_product_bundles`'s own
  `let download_units: Vec<DownloadUnit> = ...; download_units` is *not* flagged, because the explicit
  type annotation suppresses `let_and_return`.
- [ ] Module inception, still 5: `mod auth` (now nested as `client::auth::auth`, same shape as before
  under a new parent), `mod client`, `mod downloader`, `mod gogdl`, `mod secure_links`.
- [x] 1 "all variants have the same postfix: `Error`" on `FileSystemError`
  (`downloader/fs/error.rs:6-21`, `enum_variant_names`) — *no longer fires, as of the tenth pass;* the new
  `NoDiskMatchingPath` variant `2100296` added doesn't end in `Error`, so the enum's variants no longer
  share a uniform postfix and the lint stopped matching. Not a fix, just a side effect — `ClientError`
  still isn't flagged for the same reason despite also having a non-`Error`-suffixed variant among its
  six (`MaxRetriesReached`), so this was never a real signal to begin with.
- [ ] A handful of smaller one-offs (locations re-derived at `a11273a`): an explicit-closure-for-cloning
  in `depot/build_metadata.rs:38`, 3 `len_zero` (`downloader.rs:97,118,196` — one pre-existing in
  `download`, two added by `repair`), `io_other_error` at `util/hash.rs:44`, a redundant `&` in a
  `format!` call at `depot/depot_info.rs:80`, the pre-existing "redundant field names in struct
  initialization" `offset: offset` — now at `depot/depot_info.rs:58`, having moved with the code from
  `downloader/download_unit.rs:33` — and two `or_insert_with(Vec::new)` that should be `or_default()`
  (`downloader/downloadable_product.rs:50`, `downloader/product_bundle.rs:42`). The `redundant_closure`
  at the old `downloader.rs:115` is gone: it was `DownloadUnit::from_depot_file`, which `a11273a`
  deleted. There are no `redundant_closure` warnings left in the crate.

A seventh-pass update (`31809e9`/`46b6ce3`, never logged in this section before now): the count dropped
34→33. The only change was **−1** "redundant closure" at the old `downloader.rs:200` — `31809e9`'s
switch to an `AsyncFnMut` callback replaced the sync closure clippy was flagging there with a body that
directly awaits `decoder.write_all(&chunk)`, so the pattern clippy matched no longer exists. Nothing
else moved by more than a line-number shift from the file growing.

An eighth-pass update (`71d49a8`/`de52483`) brings the count 33→35, both new warnings inside the new
`stream_chunk` retry loop (`src/client/client.rs:71-135`), mirroring warnings `fetch` already had:
**+1** "redundant field names in struct initialization" at `client.rs:123` — `return
Err(ClientError::HttpError { status, body: body })` in `stream_chunk`'s non-401 arm, the same pattern
`fetch`'s arm at `:58` already has (now three total in the crate alongside `download_unit.rs:33`); **+1**
"this expression creates a reference which is immediately dereferenced by the compiler" at `client.rs:127`
— `stream_chunk`'s own `AuthError::TokenExpired` arm calls `self.auth_manager.refresh_auth(&self)`, the
same needless-borrow `fetch` has at `:55`/`:62` (now three occurrences of that one, not two). `71d49a8`
itself added no new warnings — `Downloader::verify`'s new early return is plain, unflagged code.

A ninth-pass update (`e4a8560`) leaves the count at 35 — unchanged. `stream_chunk` grew by 9 lines
(`71-135`→`71-144`) adding the `attempts < 3` retry branch on the non-401 `HttpError` arm and the whole
new `ClientError::NetworkError` arm, which pushed the two warnings logged above down to `client.rs:126`
and `:130` respectively; neither new branch introduced a warning of its own (`if attempts < 3 { continue;
}` and `return Err(ClientError::NetworkError(err))` are both plain, unflagged code).

A tenth-pass update (`2100296`) moves the count 35→36. **−1** `enum_variant_names` on `FileSystemError`
— gone, per the updated bullet above, a side effect of the new `NoDiskMatchingPath` variant rather than a
fix. **+1** `manual_map` on the new `DepotFile::size` (`depot/depot_info.rs:36-40`) — its `if let
Some(chunks) = &self.chunks { Some(...) } else { None }` should be
`self.chunks.as_ref().map(|chunks| ...)`. **+1** `needless_return` on the new
`PathResolver::get_free_space`'s `return Err(FileSystemError::NoDiskMatchingPath(...))`
(`downloader/fs/path_resolver.rs:49-51`) — sitting inside a `match` arm where a bare expression would do,
same lint category as the three other `needless_return`s already in the crate (`client.rs:175,178`,
`downloader.rs:451`, none of which this doc had previously called out by name — folded into "a handful of
smaller one-offs" above until now). Deleting `ChecksumAlgorithm`/`sha2` introduced no warnings of its own.

An eleventh-pass update covers two steps at once, since the three commits between `2100296` and
`v0.0.9-restart` were never logged here. **36→37** across `d679048`/`51b7a53`/`8e8a392`: **+1**
`manual_map` at `client/auth/auth_manager.rs:113` — `51b7a53`'s "clone the observer out of the guard"
fix writes `if let Some(observer) = &inner.token_observer { Some(observer.clone()) } else { None }`,
which should be `inner.token_observer.as_ref().map(|observer| observer.clone())` (or just `.cloned()`).
Nothing else changed. Then **37→39** across `a11273a`: **+2** `len_zero` in the new `repair`
(`downloader.rs:97` `files_allocation_error.len() != 0`, `:118` `missing_units.len() == 0`, both copies
of the idiom `download` already had at `:196`); **+1** `let_and_return` in the new
`DownloadManager::repair_game` (`download_manager.rs:100`, the third copy of the same
`let links = { .. let links = ..; links }` block); **+1** `redundant_field_names` at
`depot/depot_info.rs:58` and **−1** at `download_unit.rs:33` — the same `offset: offset`, relocated
with the code; **−1** `redundant_closure` at the old `downloader.rs:115`, since
`DownloadUnit::from_depot_file` no longer exists. Every one of the four net-new warnings is in code
`a11273a` copied rather than wrote, which is the clippy-visible shadow of the duplication item above.
`cargo build --lib` remains free of rustc warnings.

Run `cargo clippy --fix --lib -p gogdl-lib -- -W clippy::all` for a current full list and to
auto-apply most of these — but only against a clean tree; it will also try to "fix" whatever's
mid-edit if run against uncommitted work.
