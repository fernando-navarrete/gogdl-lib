# GAPS.md

Open findings for `gogdl-lib`. Current tree: HEAD **`e4596b6`** ("Refine cloud save authentication
and listing") on `feature/saves`, tagged **`v1.0.4`**. The `restart` branch has since been merged
into `main`, which sits at `8682797` (`v1.0.1`); `feature/saves` is nine commits ahead of it (the
cloud saves work plus the `v1.0.2`–`v1.0.4` bumps). **`lumen-cli` (sibling repo) is pinned to
`v1.0.4`** (`Cargo.toml:12`, `Cargo.lock` source `tag=v1.0.4#e4596b6`), a tag reachable only from
`feature/saves` (and `origin/feature/saves`), not from `main`. Merge before retiring that branch.
This pass did not rebuild `lumen-cli` against this tree.

**What changed since the last pass of this document (`66105ed`, on top of `v0.0.13-restart`),
39 commits in all:**

1. **Proton-GE support** (`7d73422`, `ea961bb`, `767f4ff`, `410000f`, `34a2c52`, `51cb054`,
   `b9a3e88`, `2f9bbe1`, `121b906`, `8682797`). New `src/proton/` module:
   `GogDl::get_proton_releases`, `get_proton_release_by_tag` and `download_proton_release`, which
   streams a GitHub release tarball straight through gzip and `tar` into place. New `tar` and
   `tokio-util` dependencies.
2. **Cloud saves, listing only** (`2ad6a74` → `e4596b6`). New `src/saves/` module:
   `GogDl::get_save_files` exchanges the session refresh token for a game-scoped grant using the
   build metadata's `client_id`/`client_secret`, then lists `cloudstorage.gog.com`. Grants and
   credentials are cached per `(game_id, build_name)`. No download or upload yet.
3. **Downloader plumbing.** `004e68e` moved secure-link resolution out of `HttpClient::stream_chunk`
   and into `download_files`'s retry loop, deleting `ClientError::SecureLinksError { inner: String }`
   in favor of the structured `DownloadError::SecureLinksError`. `c8170e2` gave
   `ChunkHashMismatch` its detail. `f4d7388` wraps the zlib decoder in a `Mutex` behind a
   `BoxFuture` callback so `stream_chunk` can be shared with the Proton downloader. `cb2d6a1`
   changed the verify/allocate helpers to take owned `Vec`s and `Arc<PathResolver>`. `fs/` moved
   from `downloader::fs` to the crate root.
4. **Client.** `7a57534` added a `headers: Option<&[(&str, &str)]>` parameter to
   `fetch`/`fetch_no_retry` (every existing call site passes `None`). `2ad6a74` added
   `HttpClient::get_refresh_token`.
5. **Misc.** `7d480d5` changed the language filter to `["en-US", "en"]`. `e0353db` removed an
   unused languages module. `92030cf` → `172f92b` moved the crate to `1.0.x` versioning.

Line references below were re-derived against `e4596b6`. Fixed items are collapsed to one line
each in [Closed](#closed) at the bottom; the detail lives in the referenced commits.

**Two standing facts that apply to almost every item here:**

- **No automated tests exist anywhere** (`grep -rn '#\[test\]\|#\[tokio::test\]' src/` is empty, and
  there is no `tests/`). Treat every fix in this doc as unverified. This is still the direct reason
  the "a failed chunk is reported as success" bug shipped four times on the old `restart` line. The
  two new modules (Proton, saves) arrived with none either, and `004e68e` restructured the retry
  loop with two new sentinel sites and nothing to pin them.
- **`v0.0.12-restart` still carries the retry off-by-one** fixed in `2469b13`. Nothing current
  resolves it (`lumen-cli` is on `v1.0.4`), but the tag still exists.

`cargo build --lib` is **no longer free of rustc warnings**. There is one: `field inner is never
read` on `ProtonManager` (`src/proton/proton_manager.rs:20`), see [Low](#low--style--clippy).
`cargo clippy --lib -- -W clippy::all` reports **43 clippy warnings** (44 with that `dead_code`),
the same count as the previous pass but a different mix: two new `unnecessary_cast`s from the new
secure-link retry sites, three new warnings in `saves/`, and three `redundant_field_names`/
redefinition warnings gone. See the [clippy](#low--style--clippy) section.

---

## High — downloader reliability

- [ ] **The per-failure-class budget is right; the batch still can't short-circuit.** With
  `attempt < MAX_ATTEMPTS - 4` (`src/downloader/downloader.rs:371`), a size-correct digest mismatch
  costs 3 transfers rather than 6, and the `remaining() != 0` arm (`:384-401`) keeps all six. What
  is unchanged is that nothing short-circuits: `download_files` drains the whole `buffer_unordered`
  stream and only then collects the results (`:407-411`), so the first chunk's three failed attempts
  don't stop the remaining units from spending theirs. Against a stale manifest that is 3× the
  transfer for every affected chunk, discovered one chunk at a time. A first `ChunkHashMismatch` is
  evidence about the *manifest*, not about that one chunk, and could reasonably abort the batch.

- [ ] **`DownloadEvent::Progress` is emitted once per network read, the event-rate problem mainline
  `v0.1.1` was cut to fix. It is now on three code paths.** `download_files`'s callback sends
  `DownloadEvent::Progress(chunk_lenght)` for every `Bytes` the `bytes_stream()` yields
  (`src/downloader/downloader.rs:310-312`) into an unbounded channel. A forwarding task re-sends it
  into another unbounded channel (`:211-217` for `download`, `:131-137` for `repair`). With
  `self.threads` chunks in flight on a fast connection that is thousands of sends per second,
  bounded by read syscall size rather than by throughput. Since `f4d7388`, each of those reads also
  pays a `Box::pin` allocation and a `Mutex::lock().await` (see Low).
  `download_proton_release` copies the same shape (`src/proton/proton_downloader.rs:88-89`,
  `ProtonDownloadEvent::Progress(chunk.len())` per read). It is a single stream, so the rate is
  lower, but the channel is still unbounded.

  The workspace `CLAUDE.md` documents this same shape as the repair-download memory leak that
  `master`'s `v0.1.1` exists to fix. There, `gogdl_flutter`'s drain loops flooded an unbounded
  `StreamSink` faster than Dart could drain it. The fix had two halves: coalescing in the bridge,
  *and* cutting the rate at the source by moving `Progress` to the write-buffer flush boundary. This
  line has neither half. Fix before any Flutter consumer is wired up to `repair_game`.

## Medium — credentials, saves & Proton

- [ ] **Refresh tokens are sent in URL query strings, and `reqwest::Error`'s `Display` prints the
  URL, so a network failure puts the token in an error string.** `reqwest` 0.13.4's `Error`
  formats as `... for url (<full url>)` (`reqwest-0.13.4/src/error.rs:280`) unless
  `.without_url()` is called, and nothing in this crate calls it. Two paths put a live refresh token
  in that URL:
  - `AuthManager::refresh_auth` builds `{REFRESH_URL}&refresh_token={refresh_token}` and stringifies
    any non-auth failure into `AuthError::ClientError { inner: e.to_string() }`
    (`src/client/auth/auth_manager.rs:99-107`). A `NetworkError` there carries the refresh token
    into a `pub` string field. This predates this pass but was never recorded.
  - New with `2ad6a74`: `SavesAuth::get_saves_auth` builds
    `auth.gog.com/token?client_id=..&client_secret=..&grant_type=refresh_token&refresh_token=..`
    (`src/saves/saves_auth.rs:66-73`) and propagates the `ClientError` as
    `SavesError::ClientError`. Its `Display` includes the session refresh token *and* the game's
    `client_secret`.

  A consumer that logs `GogDlError`/`SavesError` with `{}` writes the session's long-lived
  credential to its log. The crate's own `client_secret` in `REFRESH_URL` is a
  public constant, so only the refresh token matters there. Strip the URL (`err.without_url()`)
  where a token-bearing request's error is converted, or POST the grant as a form body if GOG's
  token endpoint accepts it. Neither query value is percent-encoded either, though GOG's token
  alphabet doesn't currently need it.

- [ ] **`SavesManager` holds its `inner` mutex across network round-trips, the same shape fixed for
  auth in `9a1f780`.** `GameSaveIds::get_game_save_ids` takes `saves_manager.inner.lock()` and
  awaits `inner.games.get_game_builds(game_id)` while holding it (`src/saves/game_save_ids.rs:51-52`),
  then does the same for `inner.depot.get_build_metadata(..)` (`:61-62`). `SavesAuth::get_saves_auth`'s
  cache lookup (`saves_auth.rs`) and every other `get_save_files` call need that lock too, so one
  game's cold build-list and metadata fetch stalls cloud-save calls for every other game. The
  method's doc comment describes this lock-holding but doesn't flag the cost. It doesn't need the
  lock at all: `DepotManager` and `GamesManager` are cheap `Clone` handles over their own
  `Arc<Mutex<..>>`, so they can live on `SavesManager` directly, outside the mutex, leaving
  `SavesManagerInner` holding only the two `HashMap`s. `SecureLinksManager::get_secure_links` has the
  same pattern around `get_owned_games()` (`src/secure_links/links_manager.rs:61-65`). That one is
  older, but saves copied it.

- [ ] **`get_save_files` requires a still-valid session access token and never refreshes it; a
  revoked game grant stays cached until it expires.** `HttpClient::get_refresh_token`
  (`src/client/client.rs:254`) goes through `AuthManager::get_auth`, which returns
  `AuthError::TokenExpired` once the *access* token lapses. So an app that restores auth and calls
  `get_save_files` first fails, even though the refresh token it actually needs is fine. Every
  other authenticated `GogDl` method refreshes in that case via `fetch`. On the storage side, the
  listing uses `fetch_no_retry` with a hand-built `Authorization` header
  (`src/saves/save_files.rs:53-62`), so a 401 surfaces as a plain `HttpError` without evicting the
  cached `SavesAuth`. Every call returns the same 401 until `valid_until` passes. Both behaviors are
  stated in `GogDl::get_save_files`'s rustdoc, so this is documented, not fixed.

- [ ] **`get_save_files` returns `SavesError`, not `GogDlError`, contradicting the crate docs.**
  `src/lib.rs:27` says "Every fallible [`GogDl`] method returns [`GogDlError`]", and `GogDlError`
  gained a `ProtonError` variant for exactly this reason (`src/gogdl/error.rs`), but
  `GogDl::get_save_files` (`src/gogdl/gogdl.rs:471`) returns `Result<Vec<SaveFile>, SavesError>`, and
  `GogDlError` has no saves variant. A consumer with one `GogDlError` error path can't `?` it. Add
  `GogDlError::SavesError(#[from] SavesError)` and return that, or amend the crate docs.

- [ ] **Proton tarballs are extracted without any integrity check.** Every Proton-GE release ships a
  detached `.sha512sum` asset per tarball, and GitHub's asset object carries a `sha256:` `digest`,
  but `GithubAsset` deserializes neither (its doc comment says so, `src/proton/github_asset.rs`). The
  stream is extracted as it arrives (`proton_downloader.rs`), so a truncated or corrupted transfer
  that still parses as gzip+tar lands on disk as a runnable Proton tree with no error. Hashing while
  streaming (the same `HashingWriter` idea the chunk downloader uses) and comparing against the
  `.sha512sum` asset before returning `Ok` would close it. Because extraction happens during the
  transfer, a mismatch still needs the caller to clean up, which `download_proton_release`'s
  "partial tree" docs already cover.

- [ ] **`HttpClient::fetch` spins on a 401 when `require_auth` is `false`: six immediate retries, no
  backoff, then the status is discarded.** The `HttpError` arm (`src/client/client.rs:51-60`) only
  refreshes when `require_auth` is true, and otherwise falls out of the `match` with no `continue`,
  no `backoff` and no `return`. The loop re-sends the identical request `MAX_ATTEMPTS` times back to
  back and then returns `ClientError::MaxRetriesReached`, losing both the 401 and its body. This
  dates from `9cd8d0a`, but it is now reachable against a third-party API: both GitHub fetches go
  through `fetch(.., false, false, ..)` (`proton_ge_release.rs`, `proton_ge_releases_page.rs`), and
  `GogDl::get_proton_releases`' rustdoc (`gogdl.rs:86-89`) describes the fetch as one that "only
  retries a 401 or a transport error", which reads as deliberate handling. On the unauthenticated
  path, a 401 should return `HttpError` immediately like every other non-success status.

- [ ] **The Proton and crate-level docs describe a `User-Agent` requirement the code no longer has,
  and the crate overview predates two of the three new features.** `7a57534` made both GitHub API
  fetches send `("User-Agent", "gogdl")` per request (`proton_ge_release.rs:80`,
  `proton_ge_releases_page.rs:42`). Yet `GogDl::get_proton_releases`/`get_proton_release_by_tag`
  (`gogdl.rs:76-80,107-110`), `ProtonError::ClientError` (`proton/error.rs:17`) and the crate docs
  (`lib.rs:38-42`) still tell consumers their `reqwest::Client` *must* set one or get a 403. `lib.rs`
  also still calls `get_proton_releases` "the one method that doesn't talk to GOG" (`:37`), though
  there are now three. Its first line (`:1-3`) and its error-type list don't mention cloud saves or
  `SavesError`. `#![warn(missing_docs)]` only catches missing docs, not stale ones.

## Medium — duplication & consistency

- [ ] **`MAX_ATTEMPTS` is one constant, but the retry primitives are still split across two modules
  and two import paths, and there are now more hand-written sentinels.** `src/constants/mod.rs:5`
  holds `pub const MAX_ATTEMPTS: u32 = 6;`. `004e68e`'s two secure-link retry sites bring the count
  to seven `attempt != MAX_ATTEMPTS - 1` sentinels (`downloader.rs:281,293,322,333,352,386`,
  `client.rs:65`) plus the MD5 arm's `attempt < MAX_ATTEMPTS - 4` (`downloader.rs:371`). Three
  residuals:
  - *The constant and the function that consumes it live in different modules.* `MAX_ATTEMPTS` is in
    `constants`, next to endpoint URLs it has nothing to do with. `backoff` is in `downloader::util`,
    imported as `crate::downloader::backoff` by `client.rs:16` and as
    `crate::downloader::util::backoff` by `downloader.rs:12`: two paths to one item, and the client
    layer depends on the downloader for a generic retry primitive. (`004e68e` did drop the client's
    other `downloader` import, `FileType`.) A `retry` module owning both, with a doc comment
    stating the base, ceiling and jitter policy, would remove both oddities.
  - *A per-failure-class budget needs a per-failure-class constant.* `MAX_ATTEMPTS - 4` reads as
    "four fewer than the transport budget" and means "three attempts". `MAX_ATTEMPTS = 5` gives the
    MD5 arm 2, `4` gives it 1, and anything below 4 stops compiling. A named
    `MAX_HASH_ATTEMPTS: u32 = 3` with `if attempt + 1 < MAX_HASH_ATTEMPTS` states it where it's read.
    This is a legibility fix, not a correctness one.
  - *`backoff`'s 20s ceiling still never binds.* `src/downloader/util/backoff.rs:4-6`; the largest
    `attempt` it sees is 4, so the largest reachable ceiling is 8s. The de-facto per-chunk timeout
    against a dead CDN (five backoffs, ~7.75s average, 15.5s worst case) is still recorded nowhere.

- [ ] **The retry loops are hand-copied rather than shared, and `download_files` still carries a dead
  arm, now more obviously dead.** `download_files`'s `Err(ClientError::AuthError(err))` arm
  (`src/downloader/downloader.rs:319-327`) is unreachable. Since `004e68e`, `stream_chunk` takes a
  bare, pre-signed URL (`client.rs:78`) and never touches `AuthManager`, and secure-link auth
  failures now arrive earlier as `DownloadError::SecureLinksError` (`:277-287`). The arm still
  invalidates, backs off and emits `ProgressRegression`, all on a path that cannot execute.

  The `HttpError` arm still tests `status == UNAUTHORIZED` twice (`:330` to invalidate, `:334` to
  skip the backoff), with `continue` written out in both branches (`:335`, `:338`). The
  invalidate/back-off/`continue`-or-`return` recovery shape is now spelled out separately at
  **eight** sites: the secure-link fetch (`:277-287`), the URL pick (`:289-299`), the three
  retrying `stream_result` arms, the two verification arms, and `HttpClient::fetch`. There are
  eight hand-pasted `ProgressRegression` sends besides (`:320,329,343,347,351,362,370,385`). A single
  `retry_or_return(attempt, bound)` helper would make each site one line and put each sentinel in
  exactly one place. The silent-success regression got in the last time a copy's *guard* was
  touched.

- [ ] **Deterministic secure-link failures are retried six times with backoff. They are now
  structured, so this is fixable.** `SecureLinksManager::get_secure_links` can fail with
  `IncorrectGameId` (unparseable ID) or `ProductNotOwned` (`links_manager.rs:67-74`). Neither will
  change on retry, but `download_files`' secure-link arm (`downloader.rs:277-287`) invalidates,
  backs off and retries all of them the same way, ~7.75s on average per chunk before failing, times
  every in-flight chunk. Before `004e68e` this error was a `String` inside
  `ClientError::SecureLinksError` and couldn't be told apart. It is now a real `SecureLinksError`,
  so a `match` can return immediately on the two deterministic variants, as `7eb5d5e` did for local
  I/O errors.

- [ ] **`Downloader::repair` is a near-verbatim copy of `Downloader::download`.** `repair`
  (`src/downloader/downloader.rs:45-141`) and `download` (`:142-221`) are the same function apart from
  one inserted stage. Lines `51-105` of `repair` and `148-202` of `download` (path resolver, the
  `depot_files` flat_map, size verification with its channel/`tokio::join!`/forwarding boilerplate,
  the free-space check, allocation and the `FileAllocationError` abort) are identical. The only real
  difference is `repair`'s `verify_download_units` stage (`:107-119`), whose `missing_units` it
  passes to `download_files` where `download` passes every unit (`:207`). `cb2d6a1` touched both
  copies identically, which shows the maintenance cost. `download` is expressible as `repair` with
  verification skipped, or both as a shared helper taking a "which units" closure.

- [ ] **`download` re-downloads every chunk regardless of what's already correct on disk, so `repair`
  is the crate's only resume path.** `download` computes `missing_files`, then builds the transfer
  list from `DownloadUnit::from_product_bundles(bundles)` (`src/downloader/downloader.rs:207`), which
  covers every chunk of every file, including files that just verified as complete. Behavior
  unchanged. The rustdoc on `GogDl::download_game`/`repair_game` says which one resumes.

- [ ] **`repair` checksums the chunks of files it has just allocated.** Stage 2 allocates every file
  that failed size verification (`set_len`, `src/fs/path_resolver.rs:81`), and stage 3 then MD5s
  **every** unit of **every** file (`src/downloader/downloader.rs:107-119`), including all-zero
  ranges that cannot match. Filter `missing_files`' units out of verification and add them straight
  to the download list.

- [ ] **Secure-link fetches aren't collapsed across concurrent chunk downloads.**
  `SecureLinksManager::get_secure_links` (`src/secure_links/links_manager.rs:51-85`; cache at `:27`)
  has no in-flight dedup. `download_files` runs up to `self.threads` units concurrently
  (`downloader.rs:407`), and since `004e68e` each attempt of each unit calls `get_secure_links`
  itself at `:277`.
  - *Cold start:* the first `self.threads` tasks all miss the empty cache at once and each issues
    its own round-trip, plus its own `get_owned_games()` check, since cache check and fetch aren't
    one critical section. This happens per product bundle, on every download and repair.
  - *Expiry:* a cache hit checks `SecureLinks::is_valid` first, but N chunks racing past the same
    deadline each miss independently. The two verification retries (`:372`, `:387`) and the CDN-401
    arm (`:331`) also invalidate reactively, so a batch of chunks failing together produces the same
    storm. `004e68e` added two more invalidate call sites (`:280`, `:292`), so a failed fetch
    storm-invalidates too.

  An in-flight dedup (a per-`game_id` `OnceCell`/shared future, the shape `PathResolver::dir_cache`
  uses at `src/fs/path_resolver.rs:13,93`) fixes both halves.

- [ ] **Types a consumer must be able to name are not exported: `DepotFile`, `Chunk`,
  `DownloadUnit`, and now `FileSystemError`.** The exported `ProductBundle` declares
  `pub product_files: Vec<DepotFile>` (`src/downloader/product_bundle.rs:23`), and `DepotFile.chunks`
  is `Option<Vec<Chunk>>`. None of the three is re-exported from `src/lib.rs`. `FileSystemError` is
  also crate-private (`mod fs` is private in `lib.rs`) but is the payload of two public variants:
  `DownloadError::FileSystemError` (`downloader/error.rs:79`) and, new with `410000f`,
  `ProtonError::FileSystemError` (`proton/error.rs:46`). A consumer can't match on *which* filesystem
  failure (e.g. `NoDiskMatchingPath`) happened, only format it. Both variants' doc comments say so.
  Re-export them, or narrow the public fields and payloads that leak them.

- [ ] **A dead refresh token and a routine expiry both surface as the same opaque, unstructured
  error.** `login_with_code` (`src/client/auth/auth_manager.rs:50-58`) and `refresh_auth`
  (`:99-107`) call `fetch_no_retry(&url, false, false, None)` and match
  `Err(ClientError::AuthError(e))` first. With `require_auth` hardcoded `false`, `inner_fetch` can
  never produce that variant, so every real failure, including a 401 for a dead refresh token, falls
  to `AuthError::ClientError { inner: e.to_string() }`. That string also carries the refresh token
  on a network error (see the credentials item above). `get_save_files` inherits the problem: a
  revoked refresh token there is an undifferentiated `HttpError` from the grant exchange. This needs
  a status-aware `AuthError` variant, or at minimum deleting the dead arm.

- [ ] **Five dead variants per error enum.** `Http`/`UrlParseError`/`NetworkError`/`DecodeError`/
  `DeflateError` on `DepotError`, `GamesError` and `SecureLinksError`, and the first four on
  `DownloadError`, are unconstructible. Every network call goes through `HttpClient` and arrives as
  `XError::ClientError(..)`. `DownloadError::DeflateError` is the exception: `downloader.rs`
  constructs it directly (`:272,363,391`). Because these enums are `pub`, `dead_code` never warns.
  The two new enums, `ProtonError` and `SavesError`, have no dead variants.

- [ ] **"Not a game" handling is inconsistent across near-identical fetchers.** Only
  `GameDetails::get_game_details` turns a cached `None` into `GamesError::ProductNotAGame`.
  `GameBuilds`, `GameLinks`, `GameSummary` and `OwnedGames` don't, so e.g. `get_game_builds` for a
  DLC surfaces a raw decode error. `get_save_files` inherits this through its `get_game_builds` call.

- [ ] **Caching is inconsistent between depot data of similar shape.** `DepotManagerInner` caches
  `ProductDetails` (`src/depot/depot_manager.rs:20`) but not `DepotInfo`, the per-depot manifest and
  likely the largest payload. `BuildMetadata` isn't cached either. `SavesManager` works around that
  by caching the two fields it needs (`ids_cache`), while every `get_product_bundles` call still
  re-fetches it.

- [ ] **Language and OS are hardcoded with no selection surface.** Narrowed by `7d480d5`:
  `BuildMetadata::filter_languages` now takes a slice and is called with `&["en-US", "en"]`
  (`src/depot/build_metadata.rs:36`), so depots tagged `en` are no longer dropped. There is still no
  way for a caller to choose. `GameBuilds::get_game_builds` always queries `os/windows/builds`
  (`src/games/game_build.rs:54`), which is documented as a deliberate limitation (`game_build.rs:24`,
  `gogdl.rs:223`). Cloud saves inherit it: `build_name` can only match a Windows build.

- [ ] **`GameScreenshots::resolve_links` picks a magic formatter index.** `formatter.get(2)`
  (`src/games/game_screenshots.rs:87`), with no explanation, silently dropping the screenshot if
  fewer than 3 formatters exist.

- [ ] **`DownloadEvent::Progress` reports compressed wire bytes while every sizing event reports
  uncompressed bytes.** `download_files` sums raw zlib `Bytes` frame lengths into `Progress`
  (`src/downloader/downloader.rs:310-312`), while `DownloadUnit.size` and every sizing event payload
  are uncompressed. A percentage built from them never reaches 100%, and no compressed total is
  exposed. Documented on the variant; behavior unchanged. `ProtonDownloadEvent` gets this right (both
  sides compressed) and its doc comment points at the contrast.

- [ ] **`GameDetails.id` and `GameBuilds.game_title` are `#[serde(skip)]` and never assigned.**
  They are always `0` (`src/games/game_details.rs:14-15`) and `""` (`src/games/game_build.rs:30-31`).
  Documented, unchanged. Populate them or drop them.

## Medium — missing coverage

- [ ] **No automated tests anywhere in the crate.** Highest-value first:
  - **One `#[tokio::test]` driving `download_files` against a scripted chunk source.** Since
    `004e68e`, `stream_chunk` takes a plain URL, so a local HTTP server fixture now drives the whole
    loop with no secure-link mocking beyond the manager.
    - *Fails transport on attempts 0–4, then serves wrong-MD5 bytes on attempt 5* → asserts `Err`.
      This is the only thing that pins `attempt < MAX_ATTEMPTS - 4`. Write it first.
    - *Fails every attempt* → asserts `Err`.
    - *Wrong-MD5 on the first attempt, good bytes on the second* → pins the MD5 retry and, via the
      net reported byte total, `ProgressRegression`'s arithmetic.
    - *Secure-link fetch fails with `ProductNotOwned`* → pins whatever the fix for the retry item
      above decides.
  - **`backoff`'s bounds.** The attempt → ceiling mapping is a plain table test.
  - **Auth logic, all testable without network:** `Auth::is_valid` and `SavesAuth::is_valid`
    boundaries (both currently have the wrong-sign margin, see Low), the `to_string`/`from_string`
    round-trip, `refresh_auth` persisting `valid_until`, concurrent refresh collapsing.
  - **`SaveFile` and `ProtonGeRelease`/`ProtonGeReleasesPage` deserialization** against captured
    responses, plus `ProtonGeRelease::get_suitable_asset` against a release with `aarch64` and
    `.sha512sum` siblings. That selection logic has been rewritten three times (`34a2c52`, `51cb054`,
    `8682797`, "Select Non-ARM Proton GE Assets") with nothing pinning it.
  - **`download_proton_release`'s error attribution** (`is_pipe_closed_by_reader` preferring the
    extraction error) against a truncated gzip stream.

  `lumen-cli` still has a `#[cfg(test)] mod tests` for `apply_download_stage_event`, so the
  consumer of these events is better covered than the producer.

- [ ] **No `CLAUDE.md`/`api.md` in this repo.** `master` had both as canonical specs for
  `gogdl_flutter`. Rustdoc (enforced by `#![warn(missing_docs)]`) now covers per-item semantics, but
  there's still no single tracker of which capabilities exist. That gap is widening: Proton and cloud
  saves are both new public surface, and cloud saves is list-only so far.

## Low — style / clippy

- [ ] **`Auth::is_valid`'s 60s margin has the wrong sign, and `SavesAuth::is_valid` copied it.**
  `valid_until.map_or(false, |t| t > now - 60)` (`src/client/auth/auth.rs:68-71`) treats a token as
  usable for 60 seconds *past* expiry, the opposite of its doc comment. `2ad6a74` copied the
  expression verbatim into `SavesAuth::is_valid` (`src/saves/saves_auth.rs:88-91`), whose doc
  comment says the margin is "subtracted from `valid_until`", which the code does not do. So a
  near-expiry game grant is reused past its deadline and fails with a 401 that, per the saves item
  above, isn't evicted. `CdnUrlParams::is_valid` (`secure_links.rs:173-177`) is still the only one of
  the three with the intended direction. Fix both together (`t - 60 > now`), ideally as one helper.

- [ ] **`ProtonManager.inner` is never read, and it is the crate's only rustc warning.**
  `ProtonManagerInner {}` (`src/proton/proton_manager.rs:20,26`) is a documented placeholder for
  caching that doesn't exist. Delete it until there's something to cache.

- [ ] **The download hot path now allocates and locks once per network read.** To share
  `stream_chunk` with the Proton downloader, `f4d7388` changed its callback from
  `AsyncFnMut(Bytes)` to `FnMut(Bytes) -> BoxFuture`, which can't hold a `&mut` to the decoder across
  the returned future. The decoder therefore lives in a `tokio::sync::Mutex`
  (`downloader.rs:275,307-314`), and every read `Box::pin`s a future that awaits an always-uncontended
  lock (`proton_downloader.rs:84-91` repeats it with the duplex writer). It's small next to zlib and
  MD5, but it's paid thousands of times per second on the same path the High `Progress` item
  describes. A generic `AsyncWrite` sink parameter on `stream_chunk`, instead of a callback, would
  remove both the box and the lock.

- [ ] **Exits from the attempt body that don't emit `ProgressRegression`: now four, all at zero.**
  `open_file`'s `?` (`downloader.rs:267`), `OffsetWriter::new`'s `?` (`:272`, missed in earlier
  passes), and `004e68e`'s two secure-link `return Err`s (`:285`, `:297`) all run before a byte is
  reported, so there is no observable effect. The invariant should read "every exit cancels what it
  reported". Moving secure-link resolution *above* `open_file` would also stop each failed link
  fetch from opening and wrapping a file for nothing.

- [ ] **`.map(|unit| unit.clone().unwrap().clone())` clones every surviving item twice.**
  `cb2d6a1` changed `.unwrap().clone()` to this at `downloader.rs:470,559,644` (two copies with a
  "safe" comment), right after taking the `Vec` by value. For `DepotFile` each clone copies a
  `Vec<Chunk>`. `.into_iter().flatten().collect()` does the filter, the unwrap and zero clones.

- [ ] **`download_proton_release` guesses the extracted directory from the asset filename.**
  It returns `dest.join(asset.name.strip_suffix(".tar.gz"))` (`proton_downloader.rs:149`) without
  checking that the archive's top-level entry has that name. It does today for GE-Proton, but the
  returned `PathBuf` is unverified. The first `Extracted` entry's leading component is right there.

- [ ] **`HttpClient::stream_chunk` is now a generic "GET a URL and stream it" and misnamed.**
  `004e68e` removed everything chunk-specific (`client.rs:78`), and `410000f` uses it for a
  multi-hundred-MB tarball. Rename, e.g. `stream_url`.

- [ ] **The GitHub request headers are duplicated.** `Accept`/`User-Agent`/`X-GitHub-Api-Version`
  appear verbatim in `proton_ge_release.rs:77-81` and `proton_ge_releases_page.rs:39-43`. Make them a
  shared `const`.

- [ ] **Vestigial `let _ = body;` in `HttpClient::fetch`.** `client.rs:52` discards a `body` that
  `:58` then returns as `body: body`.

- [ ] **`chunk_lenght` is misspelled.** `src/downloader/downloader.rs:310-312`.

- [ ] **The free-space check discards its own error detail, now in three places.**
  `Downloader::download` (`downloader.rs:175-178`), `repair` (`:78-81`), and, new with `410000f`,
  `ProtonDownloader::download_proton_release` (`proton_downloader.rs:63-66`) each map
  `get_free_space()`'s `Err(_)` to a unit `CouldNotResolveFreeSpace`, throwing away the
  `FileSystemError::NoDiskMatchingPath(PathBuf)` that names the path (`src/fs/path_resolver.rs:54`).

- [ ] **`rand` with default features is a heavy dependency for retry jitter.** `rand = "0.10.2"`
  pulls `chacha20`/`cpufeatures`/`getrandom`, which is more than a sleep duration needs. `tar` (new)
  brings `filetime` and `xattr` (with `rustix`/`linux-raw-sys`) by default. The crate never reads
  xattrs, so `default-features = false` is worth checking there too.

- [ ] **`DownloadEvent::Preparing`/`Prepared` are emitted back-to-back with no work between them.**
  `downloader.rs:253-255`. Collapse them, or do real work in between.

- [ ] **Release tags don't all match `Cargo.toml`, and the newest aren't on `main`.** Narrowed from
  the previous "`0.1.1` collides with `master`" item (see Closed). `v1.0.1` (`8682797`) was tagged
  with `version = "1.0.0"` still in `Cargo.toml`, so a `Cargo.lock` resolved from it records the
  wrong version. `v1.0.2`–`v1.0.4` are correct but only reachable from `feature/saves`. The
  `-restart` tags (through `v0.1.12-restart`) also remain alongside the `v1.0.x` ones.

**Clippy: 43 warnings** (plus the rustc `dead_code` above, 44 total). 36 are auto-fixable.
Locations re-derived:

- [ ] **6 `unnecessary_cast`.** `backoff(attempt as u32)` on a `u32` at `client.rs:66` and
  `downloader.rs:282,294,323,337,353`. `004e68e` added two of these and removed one. The two
  verification arms (`:373`, `:388`) write bare `backoff(attempt)`.
- [ ] 7 `let_and_return`: `download_manager.rs:70,85,100`, `downloader.rs:472,561,646`,
  `product_bundle.rs:64`.
- [ ] 5 module inception: `client::auth::auth`, `client::client`, `downloader::downloader`,
  `gogdl::gogdl`, `secure_links::secure_links`.
- [ ] 4 `needless_return`: `client.rs:148,151`, `downloader.rs:634`, `fs/path_resolver.rs:54`.
- [ ] 3 `len_zero`: `downloader.rs:101,121,198`.
- [ ] 2 `redundant_field_names`: `client.rs:58` (`body: body`), `depot_info.rs:58`.
- [ ] 2 `needless_borrow` on `refresh_auth(&self)`: `client.rs:55,62`.
- [ ] 2 `manual_map`: `auth_manager.rs:113`, `depot_info.rs:39`.
- [ ] 2 `or_insert_with(Vec::new)` → `or_default()`: `downloadable_product.rs:62`,
  `product_bundle.rs:59`.
- [ ] 2 `map_or(false, ..)` → `is_some_and(..)`: `client/auth/auth.rs:69`, and its copy at
  `saves/saves_auth.rs:89` (new).
- [ ] 2 redundant `&` in `format!`: `depot/depot_info.rs:80`, `saves/save_files.rs:59` (new).
- [ ] 1 `collapsible_if`: `saves/saves_auth.rs:56` (new), the cache lookup's nested
  `if let .. { if auth.is_valid() }`. `links_manager.rs:54-56` already uses the let-chain form.
- [ ] 1 redundant redefinition of `path_resolver`: `downloader.rs:258`. `cb2d6a1` removed the
  other three.
- [ ] `OwnedGames::default()` shadows `std::default::Default` (`games/owned_games.rs:19`).
- [ ] Useless `format!` on a constant URL (`games/owned_games.rs:34`).
- [ ] One-offs: explicit closure for cloning (`depot/build_metadata.rs:40`), `io_other_error`
  (`downloader/util/hash.rs:44`).

**Clippy still catches none of this crate's real defects.** Of the new findings above (tokens in
error strings, a mutex held across network I/O, a 401 loop with no `continue`, an inverted expiry
margin copied into a second type), clippy flags only the inverted margin's `map_or` spelling, not
its direction.

---

## Closed

One line per fixed item, newest first within each group. Detail is in the referenced commits.

### Downloader reliability

*Everything above the next italic note landed after `v0.0.13-restart`, between `66105ed` and
`e4596b6`.*

- [x] **`ChunkHashMismatch()` carried no detail, and its sibling arm's message had a dead
  `"ok"/"mismatch"` branch** — `c8170e2`. Now `ChunkHashMismatch { path, offset, expected, actual }`
  (`src/downloader/error.rs`), formatted in full. The short-response arm's message prints the actual
  digest instead of the always-"ok" conditional (`downloader.rs:394-398`). Unverified: no test
  asserts the fields.
- [x] **Secure-link failures during a download were flattened to `ClientError::SecureLinksError {
  inner: String }`** — `004e68e`. Secure links are now resolved inside `download_files`' attempt
  loop and fail as `DownloadError::SecureLinksError(SecureLinksError)`. The stringly variant is
  deleted from `ClientError`. That the structured error is still retried uniformly, even for
  deterministic variants, is open above.
- [x] **`HttpClient::stream_chunk` owned secure-link resolution, coupling the client layer to
  `SecureLinksManager` and `FileType`** — `004e68e`. `stream_chunk` now takes a URL. `client.rs`
  no longer imports either. The residual `backoff` import path is open above.
- [x] **Download helpers borrowed slices and shadowed `path_resolver` references into closures** —
  `cb2d6a1`. Owned `Vec` plus `Arc<PathResolver>`, with explicit `.clone()`s. Three of the four
  "redundant redefinition" clippy warnings went with it. The double clone it introduced is open
  under Low.

*The next entries are from the passes before `v0.0.13-restart` and are all reachable from it.*

- [x] **Secure links were never proactively expired** — `66105ed`. `SecureLinks::is_valid`/
  `CdnUrlParams::is_valid` check `expires_at` with a 60s margin on every cache hit. `ttl`-only links
  still rely on a reactive 401, and concurrent fetches still aren't collapsed (open above).
- [x] Doc comments on the `secure_links` module's types and methods — `66105ed`. The module is
  private, so `missing_docs` doesn't enforce them.
- [x] **`ProgressRegression`'s doc comment settled the units question but not the retry/arithmetic
  contract** — rustdoc pass (`8580dc4`). `Progress`/`ProgressRegression` and
  `GogDl::download_game`/`repair_game` now state units, fold semantics, retry budgets and stage
  ordering.
- [x] **A chunk whose MD5 mismatch first appeared on a late attempt was reported as a successful
  download** — `a0ebf13`. `attempt < MAX_ATTEMPTS - 4` is a comparison, so the loop has no
  fall-through. This was the fourth appearance of the bug (`f3a944a` → `ac061e0` → `0..2` bound →
  `2469b13` → `04ff377`), and it is still untested.
- [x] **A zlib error in `decoder.shutdown()` leaked a full chunk's worth of reported bytes** —
  `a0ebf13`.
- [x] **The verification retries and non-retryable client-error arms never emitted
  `ProgressRegression`** — `04ff377`, completed by `a0ebf13`. That same commit's MD5 sentinel change
  is the regression in the entry above.
- [x] **`MAX_ATTEMPTS` was declared twice, once per file** — `770537c`.
- [x] **Bytes streamed by an attempt that later failed were counted as progress, with nothing to
  undo them** — `770537c`, extended by `04ff377` and `a0ebf13`.
- [x] **The MD5 and length checks were fused** — `770537c`.
- [x] **Vestigial `let _ = body;` in `download_files`' `HttpError` arm** — `770537c`.
  `HttpClient::fetch`'s copy is open above.
- [x] **A chunk failing its MD5/length check was never retried** — `527a9c1`.
- [x] **The MD5/length retry hit the same edge node immediately, with no delay and no link
  invalidation** — `527a9c1`.
- [x] **The retry delay was linear and unjittered** — `527a9c1`. Full-jitter
  `0..=min(500ms * 2^attempt, 20s)`.
- [x] **`HttpClient::fetch` slept twice per network retry** — `527a9c1`.
- [x] **The retry bound and its last-attempt sentinels were independent literals, in two loops** —
  `527a9c1`. Re-opened by `04ff377`, closed again by `a0ebf13`.
- [x] **A routine CDN 401 paid the same congestion backoff as a downed CDN** — `527a9c1`.
- [x] **Both retry loops off by one (`0..2` bound tested against `!= 2`)** — `2469b13`.
- [x] **A chunk that fails every attempt was reported as a successful download** — introduced by
  `f3a944a`, fixed in `ac061e0`, re-introduced by the `0..2` bound, closed by `2469b13`.
- [x] **The retry loop never broke, so every chunk was downloaded three times** — `ac061e0`.
- [x] **Local, non-retryable failures burned all three attempts** — `7eb5d5e`.
- [x] **No retry/backoff for transient network failures** — `2b1cd7f`, `2469b13`, `527a9c1`.
- [x] **`stream_chunk`'s retry resumed into a dirty writer** — `82c7980` + `f3a944a`.
- [x] **`stream_chunk` bypassed the `fetch`/`inner_fetch` funnel entirely** — `de52483`,
  `e4a8560`, then relocated to `download_files`.
- [x] **Secure links cached forever with no expiry handling** — reactive half in `de52483`,
  proactive half in `66105ed`.
- [x] **Blocking syscalls ran directly inside async tasks** — `31809e9` + `46b6ce3`. The Proton
  extractor's `spawn_blocking` + `SyncIoBridge` (`410000f`) is the correct exception: `tar` is a
  sync API.
- [x] **`Downloader::verify` discarded its own result** — `71d49a8`.
- [x] **One failed file allocation aborts the whole download** — resolved as intended behavior.
- [x] **Only per-chunk MD5 verified; SHA-256 support was unused dead code** — resolved by deletion
  in `2100296`.
- [x] **`ProgressRegression` and `VerificationStage` were source-breaking additions `lumen-cli`
  hadn't absorbed** — resolved by `lumen-cli`'s pin bumps, now at `v1.0.4`.

### Auth & client

- [x] **Four authenticated endpoints fetched with no auth token at all** — `22bf182`.
- [x] **`Auth` and `TokenObserver` were dropped from the crate root by `9cd8d0a`** — `2ae200d` +
  `3ed4dd0`.
- [x] **`TokenObserver` couldn't be implemented outside the crate** — `32e8786`, re-broken by
  `9cd8d0a`, re-fixed by `2ae200d` + `3ed4dd0`.
- [x] **Error detail was invisible to consumers** — `5b7ca2f` re-exports every per-layer error
  enum. `ProtonError` and `SavesError` are re-exported too.
- [x] **Local expiry was a hard failure, not a refresh-and-retry case** — `5b7ca2f`. Not applied to
  `get_save_files` (open above).
- [x] **Any non-401 HTTP error was silently retried three times with its detail discarded** —
  `5b7ca2f`. The 401-without-auth path still loops (open above).
- [x] **`refresh_auth` persisted tokens without `valid_until`** — `998829d`, regressed by `9a1f780`,
  fixed again in `5b7ca2f`.
- [x] **Restoring persisted tokens locked the app out entirely** — `6c76f03`.
- [x] **`valid_until` was dead** — `998829d`.
- [x] **`is_valid()` had no clock-skew / in-flight margin** — `d679048`. The margin's sign is wrong
  (open under Low).
- [x] **Silent, internal token refreshes were invisible to callers** — `998829d` + `b87d8dd`.
- [x] **Token observer only emitted the access token** — `b87d8dd`.
- [x] **A registered `TokenObserver` could never be replaced with "none"** — `d679048`.
- [x] **The observer callback ran while the `inner` mutex was held** — `51b7a53`.
- [x] **The `inner` mutex was held across the refresh/login network round-trip** — `9a1f780`.
  `SavesManager` reintroduced the pattern (open above).
- [x] **`refresh_lock` serialized refreshes but didn't collapse them** — token-identity snapshot
  before queueing.
- [x] **`AuthManager::set_auth` was unreachable** — resolved by removal.
- [x] **Twelve `get_auth().await.unwrap()` calls became reachable panics in `998829d`** — `9a1f780`.
- [x] **Two `.parse().unwrap()` calls could crash the process on untrusted input** — fixed on top of
  `5b7ca2f`.
- [x] **The "fetch → on 401 refresh → retry once" block was copy-pasted six times** — `9cd8d0a`.
- [x] **`DownloadError` was left out of the `a962ef7` error unification** — `9a1f780`, restructured
  by `9cd8d0a`.

### Style

- [x] **`Cargo.toml`'s `version = "0.1.1"` disagreed with every `restart` tag and collided with
  `master`'s released `v0.1.1`** — `92030cf` moved to `1.0.0`, and `v1.0.2`–`v1.0.4` each match
  their tag. `lumen-cli`'s `Cargo.lock` now records `1.0.4` for `tag=v1.0.4`. The `v1.0.1`
  exception is open under Low.
- [x] **Leftover debug `println!` in a filter closure** — `1d98eb7`.
  `grep -rn 'println!\|eprintln!\|log::\|tracing::' src/` is still empty. The Proton work
  (`b9a3e88`, "Log Proton GE release page fetches") left no logging in the tree.
- [x] **Redundant `if let None = ..` instead of `.is_none()` in five files** — `a962ef7`.
- [x] **`enum_variant_names` on `FileSystemError`** — no longer fires since `2100296`.
