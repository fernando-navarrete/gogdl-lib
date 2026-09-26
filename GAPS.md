# GAPS.md

Open findings for `gogdl-lib`. Current tree: **`main`**, `Cargo.toml` at `1.2.3`. The newest tag is **`v1.2.3`**. `v1.1.6` and
`v1.2.0`–`v1.2.3` were cut on `feat/v1.2.0-download-management` (`v1.2.0-DOWNLOAD-MANAGEMENT.md`),
which has been fast-forwarded into `main`; the `v1.1.x` line (tests, CI/CD and five patches) is also in
`main`, and its decisions are in `devlog/v1.1.0-foundation.md`. `v1.0.11` (`d5b43e6`) includes the owned-products
rename (`d6dbdc5`) and the owned-games filter (`e032b7d`). `feature/saves` was merged into `main`, so
every tag is reachable from `main`. Consumer pins: the bridge (`gogdl_flutter`) is on `v1.0.11`, and
**`lumen-cli`** is on **`v1.0.10`** (`Cargo.toml:12`), so its next bump picks up the
`get_owned_games` behavior change below with no compile error.

**What changed since the last full pass of this document (`e4596b6`, `v1.0.4`), 13 commits in all:**

1. **Cloud saves are no longer list-only** (`c52ac1a` → `abfdda3`). New public surface:
   - `GogDl::get_remote_config` returns a `RemoteConfig` (`remote-config.gog.com`, pinned to
     `component_version=2.0.43`) with `is_supported`/`get_locations`.
   - `GogDl::download_save_files` and `upload_save_files` take a Wine `prefix` plus an
     `install_path`. They expand each `<?VARIABLE?>/path` location inside the prefix
     (`src/saves/save_location.rs`), then transfer files one at a time, gzip'd and ETag/MD5-checked,
     reporting `SavesDownloadEvent`/`SavesUploadEvent`.
   - `SaveFile::relative_path_in`/`split_location` map cloud names to local paths.
   - New `SavesError` variants: `CloudStorageNotSupported`, `Io`, `FileSystemError`, `HashMismatch`,
     `InvalidHeader`, `WineUserDirNotFound`, `UnknownSaveLocationVariable`, `InvalidSaveLocation`,
     `InvalidSaveFileName`.
   - New client primitives `HttpClient::stream_chunk_with_headers` and `put_stream`.
2. **Proton extraction renames its root** (`26ea6c4`). `download_proton_release` now tracks the
   tarball's single top-level directory and renames it to the sanitized `tag_name`. If a directory
   with that name already exists, it is removed first.
3. **Download batches short-circuit** (`6b4b7f3`). `try_for_each_concurrent` replaces
   `buffer_unordered(..).collect()`, and a `ProgressGuard` (`src/downloader/util/progress_guard.rs`)
   emits `ProgressRegression` on drop. Detail is under [Closed](#closed).
4. **First unit tests** (`2457bad` onward). 39 `#[test]`/`#[tokio::test]`s, all passing
   (`cargo test --lib`), in `saves/{checksum,save_files,save_location,saves_auth,saves_uploader}.rs`
   and `proton/proton_downloader.rs`. See [missing coverage](#medium--missing-coverage) for what they
   don't reach.

**Since that pass (`8442d80`), `get_owned_games` means games only** (`d6dbdc5`, `e032b7d`):

- The old "every owned product ID" lookup is now `OwnedProducts`/`GamesManager::get_owned_products`
  (`src/games/owned_products.rs`). It is crate-internal: `GogDl::get_owned_products` and the
  `OwnedProducts` re-export are gone. Secure links and `DownloadableProduct` still use it for their
  ownership checks. `GameId` was renamed `ProductId` (still private).
- `GogDl::get_owned_games` keeps its v1.0.10 signature and `OwnedGames { owned }` shape, but now
  looks each owned product up on `gamesdb.gog.com/platforms/gog/external_releases/{id}` and keeps
  only `type == "game"` (`src/games/owned_games.rs`). DLCs, packs and similar are dropped. The
  signature didn't change, so callers get the new semantics silently. See
  [owned games](#medium--owned-games) for the gaps this opens.

Line references below were re-derived against `6b4b7f3`, except the owned-games and owned-products
references, which are against `e032b7d`. Fixed items are collapsed to one line
each in [Closed](#closed) at the bottom; the detail lives in the referenced commits.

**Two standing facts that apply to almost every item here:**

- **Coverage is partial.** As of `v1.1.0` there are 84 offline tests: `download_files` against a
  local server, `backoff`, `is_valid`, deserialization of captured responses and the public types.
  Still untested: saves transfers against a server, `refresh_auth`, `HttpClient::fetch` and
  `download_proton_release` end to end (see [missing coverage](#medium--missing-coverage)). Treat
  fixes in those paths as unverified. The gap was why the "a failed chunk is reported as success" bug shipped four times on the old `restart` line,
  and `6b4b7f3` restructured the batch driver with nothing to pin it.
- **`v0.0.12-restart` still carries the retry off-by-one** fixed in `2469b13`. Nothing current
  resolves to it (`lumen-cli` is on `v1.0.10`), but the tag still exists.

*(Fixed in `v1.1.0`: `cargo build` and `cargo clippy --all-targets -- -D warnings` are now clean.
The paragraph below is the baseline it started from.)* `cargo build --lib` showed **six warnings**: an unused `OwnedProducts` import left behind by
`e032b7d` (`src/gogdl/facade.rs:11`), plus five `dead_code`: `ProtonManager.inner`
(`src/proton/proton_manager.rs:20`) and four unused `RemoteConfig` fields (`version`, `macos`,
`overlay`, `supported` at `src/saves/remote_config.rs:20,115,123,134`). See
[Low](#low--style--clippy). `cargo clippy --lib -- -W clippy::all` reports **42 clippy warnings**
(48 with those six), 38 auto-fixable. The mix and locations are listed in the
[clippy](#low--style--clippy) section.

---

## Medium — cloud saves

- [x] **`download_save_files` truncates a good local save before it has the new one on disk.**
  `SavesDownloader::download_file` verifies and gunzips in memory, then calls
  `tokio::fs::File::create(&destination)` and `write_all`s over the existing file
  (`src/saves/saves_downloader.rs:162-166`). A failure between truncate and flush leaves a
  truncated or empty save where a working one used to be: disk full, `EIO`, a crash, or the caller
  dropping the future mid-write. Nothing is retried, so the call just returns `Io`. For save games
  that is the worst failure mode available. Write to a sibling temp file, `set_modified`, `fsync`,
  then `rename` over the destination.

  Fixed in `v1.2.0`: the write is one blocking task that creates `.<name>.gogdl-part` beside the
  destination, sets the mtime, `sync_all`s and renames it into place, with a guard that removes the
  temp file on any error. A dropped future can't interrupt it, so a save is either the old file or
  the complete new one. The uploader skips `.gogdl-part` files.

- [ ] **Upload names may not round-trip with the names download accepts.** Depending on which side
  matches GOG, the result could be duplicate cloud objects that collide on one local file.
  - The download side and `SaveFile`'s docs say games using `__default` are listed as
    `saves/__default/profile/slot1.sav` (`src/saves/save_files.rs:62-66`, pinned by
    `relative_path_in_strips_a_declared_location_after_saves`). `split_location` then maps both
    `saves/__default/x` and `__default/x` onto the same `<dir>/x` (`:98-120`).
  - The upload side always writes `<location>/<relative>` with no `saves/` (`remote_name`,
    `src/saves/saves_uploader.rs:141-143`, pinned by `remote_name_round_trips_with_relative_path_in`
    as `__default/profile/slot1.sav`).

  If GOG really lists `__default` games with the `saves/` prefix, a download→play→upload cycle
  creates a second object, `__default/…`, next to the original `saves/__default/…`. The next
  download writes both to the same local path in listing order, and the older one can win. More
  generally, `download_files` never checks that two listed names resolve to distinct destinations
  (`saves_downloader.rs:91-93`), so any such collision is a silent overwrite. The two test modules
  each pass under a different assumption about GOG's naming, and neither was checked against a live
  listing. Capture a real `__default` listing and pin both sides to it.

- [ ] **Save "sync" is a blind copy in each direction, with no way to select files.** `lib.rs:2`
  calls it "syncing cloud saves".
  - `download_save_files` downloads *every* listed file over whatever is on disk
    (`saves_downloader.rs:91-93`).
  - `upload_save_files` uploads *every* local file and overwrites whatever is in the cloud
    (`saves_uploader.rs:51-70`).
  - Neither looks at `SaveFile::last_modified`/`hash` or local mtimes, and neither propagates a
    deletion.

  Both are documented (`facade.rs:594`, `:669-671`). The gap is that neither method takes a file
  subset, so a consumer that runs its own comparison through `get_save_files` still can't act on it
  except all-or-nothing. A newer save from another machine is one wrong button away from being
  overwritten. Accept a `&[SaveFile]`/path filter, or return a plan the caller confirms.

- [ ] **Download and upload ignore `cloudStorage.enabled`.** `resolve_save_locations` calls
  `get_locations()` and only rejects an *empty* list (`src/saves/saves_manager.rs:159-164`).
  `get_locations` deliberately returns the locations of a *disabled* block
  (`remote_config.rs:83-85`), so a game whose remote config turns cloud storage off still gets read
  from and written to its storage area. Check `is_supported()` there, or document that the caller
  must.

- [x] **Refresh tokens are sent in URL query strings, and `reqwest::Error`'s `Display` prints the
  URL, so a network failure puts the token in an error string.** `reqwest` 0.13.4's `Error`
  formats as `... for url (<full url>)` (`reqwest-0.13.4/src/error.rs:280`) unless
  `.without_url()` is called, and nothing in this crate calls it. Two paths put a live refresh token
  in that URL:
  - `AuthManager::refresh_auth` builds `{REFRESH_URL}&refresh_token={refresh_token}`
    (`src/client/auth/auth_manager.rs:97`). It stringifies any non-auth failure into
    `AuthError::ClientError { inner: e.to_string() }` (`:99-107`), so a `NetworkError` there carries
    the refresh token into a `pub` string field.
  - `SavesAuth::get_saves_auth` builds
    `auth.gog.com/token?client_id=..&client_secret=..&grant_type=refresh_token&refresh_token=..`
    (`src/saves/saves_auth.rs:70-72`) and propagates the `ClientError` as `SavesError::ClientError`.
    Its `Display` includes the session refresh token *and* the game's `client_secret`. Every
    download and upload now goes through this exchange on a cold cache.

  A consumer that logs `GogDlError`/`SavesError` with `{}` writes the session's long-lived
  credential to its log. The crate's own `client_secret` in `REFRESH_URL` is a public constant, so
  only the refresh token matters there. Strip the URL (`err.without_url()`) where a token-bearing
  request's error is converted, or POST the grant as a form body if GOG's token endpoint accepts
  it. Neither query value is percent-encoded either, though GOG's token alphabet doesn't currently
  need it.

  Fixed in `v1.1.1`: the login, refresh and saves exchanges are POSTs with a form body (GOG's token
  endpoint answers a POST like a GET, probed with a bogus token and checked live), through
  `HttpClient::post_token`, which also strips the URL from every `reqwest::Error`. Tests pin both
  halves: no secret in the `Display`/`Debug` of a failed exchange, and no URL in a `NetworkError`.

- [ ] **`SavesManager` holds its `inner` mutex across network round-trips, the same shape fixed for
  auth in `9a1f780`.** `GameSaveIds::get_game_save_ids` takes `saves_manager.inner.lock()` and
  awaits `inner.games.get_game_builds(game_id)` while holding it
  (`src/saves/game_save_ids.rs:50-53`). It does the same for `inner.depot.get_build_metadata(..)`
  (`:60-63`). The method's doc comment now states this (`:29-32`) but not the cost.
  `SavesAuth::get_saves_auth`'s cache lookup and every other saves call need that lock. Now that
  includes `get_remote_config`, `download_save_files` and `upload_save_files`, so one game's cold
  build-list and metadata fetch stalls cloud-save calls for every other game.

  The lock isn't needed. `DepotManager` and `GamesManager` are cheap `Clone` handles over their own
  `Arc<Mutex<..>>`, so they can live on `SavesManager` directly, outside the mutex, leaving
  `SavesManagerInner` with only the two `HashMap`s. `SecureLinksManager::get_secure_links` has the
  same pattern around `get_owned_products()` (`src/secure_links/links_manager.rs:62-63`). That one is
  older, but saves copied it.

- [ ] **The saves API requires a still-valid session access token and never refreshes it, and a
  revoked game grant stays cached until it expires.**
  - `HttpClient::get_refresh_token` (`src/client/http.rs:342`) goes through
    `AuthManager::get_auth`, which returns `AuthError::TokenExpired` once the *access* token lapses.
    So an app that restores auth and then calls any saves method first fails, even though the
    refresh token it actually needs is fine. Every other authenticated `GogDl` method refreshes in
    that case via `fetch`.
  - On the storage side, listing, download and upload all send a hand-built `Authorization` header
    through non-retrying calls (`save_files.rs:145-156`, `saves_downloader.rs:117-124`,
    `saves_uploader.rs:104-123`). A 401 surfaces as a plain `HttpError` and never evicts the cached
    `SavesAuth`, so every call returns the same 401 until `valid_until` passes.

  Both behaviors are stated in the rustdoc, so this is documented, not fixed. The grant is also
  fetched once per call and reused across the whole sequential batch, so a batch can still outlive
  its grant (the wrong-sign margin that let it start on an already-expired one was fixed in `v1.1.2`).

- [ ] **The four saves methods return `SavesError`, not `GogDlError`, contradicting the crate
  docs.** `src/lib.rs:30` says "Every fallible [`GogDl`] method returns [`GogDlError`]", and its
  list of wrapped enums (`:31-33`) omits `SavesError`. But `get_save_files`, `get_remote_config`,
  `download_save_files` and `upload_save_files` (`src/gogdl/facade.rs:494,543,640,708`) all return
  `Result<_, SavesError>`, and `GogDlError` has no saves variant (`src/gogdl/error.rs`). This was
  one method last pass; it is four now. A consumer with one `GogDlError` error path can't `?` them.
  Add `GogDlError::SavesError(#[from] SavesError)`, or amend the crate docs.

  The crate docs and `GogDlError`'s rustdoc were amended in `v1.1.5` to say so. The variant is
  breaking, so it stays open for `v1.4.0` (ROADMAP).

## Medium — owned games

- [ ] **`get_owned_games` silently drops any product whose `gamesdb` lookup fails.** Each lookup's
  `Result` is filtered with `.filter(|d| d.is_ok())` (`src/games/owned_games.rs:48`), so a
  transport error after retries, a 404 for a product `gamesdb` doesn't know, a 429, or a body
  without a `type` field just removes that ID. The call still returns `Ok`. A flaky network
  therefore returns a *shorter library*, not an error, and the caller can't tell. `lumen-cli`'s
  `list_owned_games` (`../lumen-cli/src/middleware/catalog.rs:15-23`) calls this and says outright
  that silently shrinking the list on a per-title failure is "exactly the kind of thing this tool
  needs to show, not hide". The rustdoc now states the behavior, so it is documented, not fixed.
  Options: fail the whole call on the first non-404 error, or return the unresolved IDs alongside
  the games (e.g. an `unresolved: Vec<ProductId>` field on `OwnedGames`) so callers can decide.

- [ ] **Every `get_owned_games` call re-fetches one `gamesdb` record per owned product.** Only the
  underlying `OwnedProducts` list is cached. The type lookups aren't stored on `GamesManagerInner`
  (`src/games/games_manager.rs:23-29`), and they run two at a time (`buffer_unordered(2)`,
  `owned_games.rs:42`). A few-hundred-product library means a few hundred sequential-ish requests
  on every call, and `lumen-cli` then fans out another `get_game_details` per game on top. Product
  type doesn't change, so a `HashMap<ProductId, String>` (or `bool`) cache on `GamesManagerInner`,
  like the other per-ID caches there, makes repeat calls free. A failed lookup should not be
  cached, so the item above stays fixable.

- [ ] **The game/non-game decision rests on an undocumented endpoint and an exact string.**
  `gamesdb.gog.com` is not a documented GOG API, the request is unauthenticated
  (`fetch(.., require_auth: false, ..)`, `owned_games.rs:38`), and the filter is
  `produt_type == "game"` (`:50`). Any other spelling or new type string silently counts as
  non-game. `GameDetails::get_game_details` already has its own "not a game" signal
  (`GamesError::ProductNotAGame`, from `embed.gog.com/account/gameDetails`). The two can disagree,
  and nothing reconciles or tests them.

## Medium — Proton

- [x] **Re-downloading a Proton release whose tarball root already matches its tag extracts over the
  old tree instead of replacing it.** `download_proton_release` removes an existing `path/<tag>`
  only when it has to rename a differently-named root onto it (`if source != target`,
  `src/proton/proton_downloader.rs:208-213`). When the archive's root already *is*
  `GE-Proton10-4`, the usual case, extraction unpacks straight into the existing directory
  (`:154`). Files the new release dropped survive next to the new ones. The rustdoc promises the
  opposite: "a re-download always leaves a single, clean tree behind" (`:83-85`, repeated on
  `GogDl::download_proton_release`). A leftover `<root>` from an earlier failed attempt is overlaid
  the same way when names differ. Extract into a fresh staging directory under `path`, then remove
  and rename.
  *(Fixed in `v1.2.0`: extraction goes into `.gogdl-staging-<tag>-<random>` and the root replaces
  `path/<tag>` by rename; a failure or drop leaves `path` as it was.)*

- [ ] **Proton tarballs are extracted without any integrity check.** Every Proton-GE release ships a
  detached `.sha512sum` asset per tarball, and GitHub's asset object carries a `sha256:` `digest`,
  but `GithubAsset` deserializes neither (its doc comment says so, `src/proton/github_asset.rs:7`).
  The stream is extracted as it arrives, so a truncated or corrupted transfer that still parses as
  gzip+tar lands on disk as a runnable Proton tree with no error. Since `26ea6c4` it also *replaces*
  a previous good install of the same tag when the names differ. Hashing while streaming (the same
  `HashingWriter` idea the chunk downloader uses) and comparing against the `.sha512sum` asset
  before the rename would close both.

- [x] **`HttpClient::fetch` spins on a 401 when `require_auth` is `false`: six immediate retries, no
  backoff, then the status is discarded.** The `HttpError` arm (`src/client/http.rs:52-61`) only
  refreshes when `require_auth` is true. Otherwise it falls out of the `match` with no `continue`,
  no `backoff` and no `return`. The loop re-sends the identical request `MAX_ATTEMPTS` times back to
  back and then returns `ClientError::MaxRetriesReached`, losing both the 401 and its body. This
  dates from `9cd8d0a`, but it is reachable against a third-party API: both GitHub fetches go
  through `fetch(.., false, false, ..)` (`proton_ge_release.rs`, `proton_ge_releases_page.rs`).
  `GogDl::get_proton_releases`' rustdoc describes the fetch as one that "only retries a 401 or a
  transport error", which reads as deliberate handling. On the unauthenticated path, a 401 should
  return `HttpError` immediately like every other non-success status.

  Fixed in `v1.1.3`: the arm returns `HttpError { status, body }` at once unless a refresh applies
  (`require_auth`), so an unauthenticated 401 is one request. The rustdoc now says only transport
  errors are retried. Pinned by tests in `client/http.rs`.

- [x] **The crate-level and Proton docs are stale: a `User-Agent` requirement the code no longer
  has, and an overview that predates saves.**
  - *User-Agent.* `7a57534` made both GitHub API fetches send `("User-Agent", "gogdl")` per request
    (`proton_ge_release.rs:80`, `proton_ge_releases_page.rs:42`). Yet
    `GogDl::get_proton_releases`/`get_proton_release_by_tag` (`facade.rs:76-80,107-110`),
    `ProtonError::ClientError` (`proton/error.rs:17`) and the crate docs (`lib.rs:42-45`) still tell
    consumers their `reqwest::Client` *must* set one or get a 403.
  - *Crate overview.* `lib.rs:40` still calls `get_proton_releases` "the one method that doesn't
    talk to GOG", though there are three. The first line (`:1-3`) now mentions cloud saves, but
    "Long-running operations" (`:13-14`) omits `download_save_files`/`upload_save_files`, which take
    the same unbounded-channel contract, and the error list omits `SavesError`.
  - *Saves internals.* `SavesError::CloudStorageNotSupported` says it is "only ever returned by
    `RemoteConfig::get_locations`, never by a request" (`src/saves/error.rs:41-45`), but
    `resolve_save_locations` (`saves_manager.rs:162`) and `SavesDownloader::download_files`
    (`saves_downloader.rs:81-83`) return it too, and `facade.rs:619` documents that.
    `RemoteConfig::get_locations` says it is "`async` only for symmetry" (`remote_config.rs:77-81`),
    but it is a plain `fn`. `SavesManager` (`saves_manager.rs:20-23`) and `SavesAuth`
    (`saves_auth.rs:10-11`) still describe themselves as backing only `get_save_files`.

  `#![warn(missing_docs)]` only catches missing docs, not stale ones.

  Fixed in `v1.1.5`: the `User-Agent` requirement is gone from `GogDl::get_proton_releases`,
  `get_proton_release_by_tag`, `ProtonError::ClientError` and `lib.rs`; the overview covers the
  three GitHub-only methods, the saves transfers and `SavesError`; and the saves internals'
  descriptions match the code. A read of the rendered docs found one more stale claim, fixed
  the same way: `AuthError::TokenExpired` said `GogDl` callers never see it, but the saves methods
  return it.

## Medium — duplication & consistency

- [x] **`MAX_ATTEMPTS` is one constant, but the retry primitives are still split across two modules
  and two import paths.** `src/constants/mod.rs:5` holds `pub const MAX_ATTEMPTS: u32 = 6;`. There
  are seven `attempt != MAX_ATTEMPTS - 1` sentinels (`engine.rs:282,294,319,329,345,376`,
  `http.rs:66`), plus the MD5 arm's `attempt < MAX_ATTEMPTS - 4` (`engine.rs:362`). Three
  residuals:
  - *The constant and the function that consumes it live in different modules.* `MAX_ATTEMPTS` is in
    `constants`, next to endpoint URLs it has nothing to do with. `backoff` is in `downloader::util`,
    imported as `crate::downloader::backoff` by `http.rs:17` and as
    `crate::downloader::util::backoff` by `engine.rs`. That is two paths to one item, and the
    client layer depends on the downloader for a generic retry primitive. A `retry` module owning
    both, with a doc comment stating the base, ceiling and jitter policy, would remove both
    oddities.
  - *A per-failure-class budget needs a per-failure-class constant.* `MAX_ATTEMPTS - 4` reads as
    "four fewer than the transport budget" and means "three attempts". `MAX_ATTEMPTS = 5` gives the
    MD5 arm 2, `4` gives it 1, and anything below 4 stops compiling. A named
    `MAX_HASH_ATTEMPTS: u32 = 3` with `if attempt + 1 < MAX_HASH_ATTEMPTS` states it where it's read.
    This is a legibility fix, not a correctness one.
  - *`backoff`'s 20s ceiling still never binds.* `src/downloader/util/backoff.rs:4-6`: the largest
    `attempt` it sees is 4, so the largest reachable ceiling is 8s. Nothing records the de-facto
    per-chunk timeout against a dead CDN (five backoffs, ~7.75s average, 15.5s worst case).

  Fixed in `v1.2.0`: `src/client/retry.rs` owns `MAX_ATTEMPTS`, `MAX_HASH_ATTEMPTS` (3, read as
  `attempt + 1 < bound`) and `backoff`, with one import path, and its module doc states the policy
  and the de-facto per-chunk budget against a dead CDN (15.5s worst case, ~7.75s average).

- [x] **The retry loops are hand-copied rather than shared, and `download_files` still carries a dead
  arm.** `download_files`' `Err(ClientError::AuthError(err))` arm
  (`src/downloader/engine.rs:317-324`) is unreachable. `stream_chunk` takes a bare, pre-signed
  URL and never touches `AuthManager`, and secure-link auth failures arrive earlier as
  `DownloadError::SecureLinksError` (`:278-288`). The arm still invalidates and backs off on a path
  that cannot execute.

  The `HttpError` arm still tests `status == UNAUTHORIZED` twice (`:326` to invalidate, `:330` to
  skip the backoff), with `continue` written out in both branches (`:331`, `:334`). The
  invalidate/back-off/`continue`-or-`return` recovery shape is spelled out separately at **eight**
  sites: the secure-link fetch, the URL pick, the three retrying `stream_result` arms, the two
  verification arms, and `HttpClient::fetch`. (`6b4b7f3`'s `ProgressGuard` removed the eight
  hand-pasted `ProgressRegression` sends that used to sit beside them, so that half is done.) A
  single `retry_or_return(attempt, bound)` helper would make each site one line and put each
  sentinel in exactly one place. The silent-success regression got in the last time a copy's
  *guard* was touched.

  Fixed in `v1.2.0`: `retry_or_return` / `retry_now_or_return` hold the last-attempt test, and all
  eight sites use them. The dead `AuthError` arm and the double `UNAUTHORIZED` test are gone.

- [x] **Deterministic secure-link failures are retried six times with backoff, and since they are
  structured, this is fixable.** `SecureLinksManager::get_secure_links` can fail with
  `IncorrectGameId` (unparseable ID) or `ProductNotOwned` (`links_manager.rs:69,73`). Neither will
  change on retry, but `download_files`' secure-link arm (`engine.rs:278-288`) invalidates,
  backs off and retries all of them the same way, ~7.75s on average per chunk before failing.
  `6b4b7f3` limits the blast radius: the first unit to give up now cancels the batch instead of
  every in-flight chunk paying the full budget. But the first failure still takes the whole
  schedule. A `match` can return immediately on the two deterministic variants, as `7eb5d5e` did for
  local I/O errors.

  Fixed in `v1.2.0`: `IncorrectGameId` and `ProductNotOwned` return at once, with no invalidation
  or backoff. The characterization test is flipped to one lookup and no elapsed time.

- [x] **`Downloader::repair` is a near-verbatim copy of `Downloader::download`.** `repair`
  (`src/downloader/engine.rs:45-141`) and `download` (`:142-221`) are the same function apart
  from one inserted stage. Lines `51-105` of `repair` and `148-202` of `download` are identical:
  path resolver, the `depot_files` flat_map, size verification with its
  channel/`tokio::join!`/forwarding boilerplate, the free-space check, allocation and the
  `FileAllocationError` abort. The only real difference is `repair`'s `verify_download_units` stage
  (`:107-119`), whose `missing_units` it passes to `download_files` where `download` passes every
  unit (`:207`). `download` is expressible as `repair` with verification skipped, or both as a
  shared helper taking a "which units" closure.

  Fixed in `v1.2.2`: both call `Downloader::pipeline`, which takes a `Units` (`All` or `Verified`)
  and runs each stage through one `staged` helper, so stages 1-2 exist once. Tests pin each method's
  stage sequence (`download_stage_sequence`, `repair_stage_sequence`, and the complete-install pair),
  written before the refactor and unchanged after it.

- [ ] **`download` re-downloads every chunk regardless of what's already correct on disk, so `repair`
  is the crate's only resume path.** `download` computes `missing_files`, then builds the transfer
  list from `DownloadUnit::from_product_bundles(bundles)` (`src/downloader/engine.rs:207`). That
  list covers every chunk of every file, including files that just verified as complete. Behavior
  is unchanged. The rustdoc on `GogDl::download_game`/`repair_game` says which one resumes.

  Deferred to `v1.3.0` (ROADMAP): skipping verified chunks adds a `VerificationStage` to
  `download_game`'s events, a behavior change, so it can't ship in a `v1.2.x` patch. Until then
  `repair_game` is the resume path.

- [x] **`repair` checksums the chunks of files it has just allocated.** Stage 2 allocates every file
  that failed size verification (`set_len`, `src/fs/path_resolver.rs:81`). Stage 3 then MD5s
  **every** unit of **every** file (`src/downloader/engine.rs:107-119`), including all-zero
  ranges that cannot match. Filter `missing_files`' units out of verification and add them straight
  to the download list.

  Fixed in `v1.2.1`: stage 1 records each failed file's old length (0 if absent), and stage 3 sends
  the units at or past it straight to the download list with a `ChecksumMismatch` event and no MD5,
  so the event count is unchanged. Chunks below the old length of a resized file are still hashed,
  since `set_len` keeps those bytes. An all-zero chunk that used to verify is now re-fetched.

- [ ] **Secure-link fetches aren't collapsed across concurrent chunk downloads.**
  `SecureLinksManager::get_secure_links` (`src/secure_links/links_manager.rs`, cache at `:27`) has
  no in-flight dedup. `download_files` runs up to `self.threads` units concurrently
  (`engine.rs:258`), and each attempt of each unit calls `get_secure_links` itself (`:278`).
  - *Cold start:* the first `self.threads` tasks all miss the empty cache at once and each issues
    its own round-trip, plus its own `get_owned_products()` check, since cache check and fetch aren't
    one critical section. This happens per product bundle, on every download and repair.
  - *Expiry:* a cache hit checks `SecureLinks::is_valid` first, but N chunks racing past the same
    deadline each miss independently. Six sites invalidate reactively
    (`:281,293,318,327,363,377`), so a batch of chunks failing together produces the same storm.

  An in-flight dedup (a per-`game_id` `OnceCell`/shared future, the shape `PathResolver::dir_cache`
  uses at `src/fs/path_resolver.rs:13,93`) fixes both halves.

- [x] **Types a consumer must be able to name are not exported: `DepotFile`, `Chunk`,
  `DownloadUnit` and `FileSystemError`.** The exported `ProductBundle` declares
  `pub product_files: Vec<DepotFile>` (`src/downloader/product_bundle.rs:23`), and `DepotFile.chunks`
  is `Option<Vec<Chunk>>`. None of the three is re-exported from `src/lib.rs`. `FileSystemError` is
  crate-private (`mod fs` is private in `lib.rs`) but is now the payload of **three** public
  variants: `DownloadError::FileSystemError` (`downloader/error.rs:79`),
  `ProtonError::FileSystemError` (`proton/error.rs:46`) and, new, `SavesError::FileSystemError`
  (`saves/error.rs:58-59`). A consumer can't match on *which* filesystem failure happened (for
  example, a save file name that would escape its directory), only format it. All three variants'
  doc comments say so. Re-export them, or narrow the public fields and payloads that leak them.

  Fixed in `v1.2.0`: `FileSystemError`, `DepotFile` and `Chunk` are exported and documented.
  `DownloadUnit` is **not**: no public signature exposes it (only `DepotFile::to_download_units` and
  `DownloadUnit::from_product_bundles` touched it, now `pub(crate)`), so exporting it would only
  have published `FileType` and `_compressed_size`. `FileMetadataError` and `FileCreationError` are
  never returned by a public method; they're documented as such and removed in `v1.4.0`. The event
  enums, `FileSystemError` and the download, Proton and saves error enums are now
  `#[non_exhaustive]`; `src/variant_guard.rs` keeps the "a new variant is looked at" guard in-crate.

- [ ] **A dead refresh token and a routine expiry both surface as the same opaque, unstructured
  error.** `login_with_code` (`src/client/auth/auth_manager.rs:50-58`) and `refresh_auth`
  (`:99-107`) call `fetch_no_retry(&url, false, false, None)` and match
  `Err(ClientError::AuthError(e))` first. With `require_auth` hardcoded `false`, `inner_fetch` can
  never produce that variant. So every real failure, including a 401 for a dead refresh token, falls
  to `AuthError::ClientError { inner: e.to_string() }`. That string also carries the refresh token
  on a network error (see the credentials item above). The saves grant exchange inherits the
  problem: a revoked refresh token there is an undifferentiated `HttpError`. This needs a
  status-aware `AuthError` variant, or at minimum deleting the dead arm.

- [ ] **Five dead variants per error enum.** `Http`/`UrlParseError`/`NetworkError`/`DecodeError`/
  `DeflateError` on `DepotError`, `GamesError` and `SecureLinksError`, and the first four on
  `DownloadError`, can't be constructed. Every network call goes through `HttpClient` and arrives as
  `XError::ClientError(..)`. `DownloadError::DeflateError` is the exception: `engine.rs`
  constructs it directly (`:273,355,381`). Because these enums are `pub`, `dead_code` never warns.
  `ProtonError` and `SavesError` have no dead variants.

- [ ] **"Not a game" handling is inconsistent across near-identical fetchers.** Only
  `GameDetails::get_game_details` turns a cached `None` into `GamesError::ProductNotAGame`.
  `GameBuilds`, `GameLinks`, `GameSummary` and `OwnedGames` don't, so e.g. `get_game_builds` for a
  DLC surfaces a raw decode error. Every saves method inherits this through its `get_game_builds`
  call.

- [ ] **Caching is inconsistent between depot and saves data of similar shape.**
  - `DepotManagerInner` caches `ProductDetails` (`src/depot/depot_manager.rs:20`) but not
    `DepotInfo`, the per-depot manifest and likely the largest payload.
  - `BuildMetadata` isn't cached either. `SavesManager` works around that by caching the two fields
    it needs (`ids_cache`), while every `get_product_bundles` call still re-fetches it.
  - New: `RemoteConfig` is fetched fresh on every `get_remote_config`, `download_save_files` and
    `upload_save_files` call (`remote_config.rs:36-39`), though it describes the game, not the
    user.

- [ ] **Language and OS are hardcoded with no selection surface.** `BuildMetadata::filter_languages`
  is called with `&["en-US", "en"]` (`src/depot/build_metadata.rs:36`), and there is still no way
  for a caller to choose. `GameBuilds::get_game_builds` always queries `os/windows/builds`
  (`src/games/game_build.rs:54`), documented as a deliberate limitation (`game_build.rs:24`). Cloud
  saves inherit this twice over: `build_name` can only match a Windows build, and `RemoteConfig`
  reads only the Windows section (`remote_config.rs:63-73,90-99`). The saves side is consistent with
  the Wine-prefix design, but a native Linux build's saves are unreachable.

- [ ] **`GameScreenshots::resolve_links` picks a magic formatter index.** `formatter.get(2)`
  (`src/games/game_screenshots.rs:87`), with no explanation, silently drops the screenshot if fewer
  than 3 formatters exist.

- [x] **`DownloadEvent::Progress` reports compressed wire bytes while every sizing event reports
  uncompressed bytes.** `download_files` reports raw zlib `Bytes` frame lengths into `Progress`
  (`progress.report(chunk.len())`, `src/downloader/engine.rs:310`), while `DownloadUnit.size`
  and every sizing event payload are uncompressed. A percentage built from them never reaches 100%,
  and no compressed total is exposed. This is documented on the variant, and behavior is unchanged.
  `ProtonDownloadEvent` and `SavesDownloadEvent` get this right (both sides compressed).

  Fixed in `v1.2.3`: `DownloadEvent::Started { compressed_total }` is sent once per download stage,
  summed from the manifest's compressed chunk sizes over the units being transferred (only the
  missing ones for `repair_game`). `progress_nets_to_the_compressed_total_with_a_retried_chunk` pins
  that `Progress` minus `ProgressRegression` equals it.

- [ ] **`GameDetails.id` and `GameBuilds.game_title` are `#[serde(skip)]` and never assigned.**
  They are always `0` (`src/games/game_details.rs:14`) and `""` (`src/games/game_build.rs:30`).
  Documented and unchanged. Populate them or drop them.

## Medium — missing coverage

- [x] **No downstream build check.** A breaking change here showed up only when `lumen-cli` or the
  bridge bumped its pin, after the tag. Found while planning `v1.1.4`.

  Fixed in `v1.1.4`: a manual `downstream` CI job clones both consumers, points them at this
  checkout with a `[patch]` (plus `cargo update -p gogdl-lib`, without which cargo ignores the
  patch against their old `Cargo.lock`) and runs `cargo build` for `lumen-cli` and `cargo test` for
  the bridge's `rust/`. `LUMEN_CLI_REF`/`BRIDGE_REF` pick the refs; lumen-cli's `main` still pins
  `v1.0.4`, so use `feature/remote-config` until it's merged. Renaming `get_owned_games` on a
  scratch branch turned it red.

- [ ] **Saves transfers, `refresh_auth` and Proton extraction are still untested.** `v1.1.0`
  covered `download_files`, `backoff`, the `is_valid` boundaries, the `Auth` round-trip and
  deserialization of captured responses (see [Closed](#closed)). Still open, first to last:
  - **Save download/upload against a local server.** *(Fixed in `v1.2.0`: a body cut short, a drop
    mid-transfer, an ETag mismatch, a missing `X-Object-Meta-LocalLastModified` and a failed write
    are tested. Still open:)* a listing with two names that resolve to one path (see the
    round-trip item). Also a captured listing for a `__default` game (the captured one is a
    `saves` game).
  - **`refresh_auth` persisting `valid_until`, and concurrent refreshes collapsing.** Blocked on
    `REFRESH_URL` being hardcoded; needs injectable hosts (an API change, so a minor).
  - ~~**`download_proton_release` end to end:** error attribution (`is_pipe_closed_by_reader`
    preferring the extraction error) against a truncated gzip stream, and the re-download overlay in
    the Proton item above.~~ *(Fixed in `v1.2.0`: a clean install, a re-download, a truncated
    stream, a non-gzip body larger than the pipe, a dropped future and the stale sweep are tested.)*

- [ ] **The event enums and Proton types can't be compared or printed from outside the crate.**
  `DownloadEvent`, `DownloadStageEvent`, `FileAllocationEvent`, `FileSizeVerificationEvent`,
  `VerificationEvent`, `ProtonDownloadEvent`, `SavesDownloadEvent`, `SavesUploadEvent`,
  `ProtonGeRelease`, `ProtonGeReleasesPage` and `GithubAsset` derive none of `Debug`, `Clone` or
  `PartialEq`, so a consumer's test can build them (every variant and field is public, see
  `tests/public_types.rs`) but must use `matches!` instead of `assert_eq!`/`{:?}`. Found in
  `v1.1.0` (Phase 4). Adding the derives is additive, so it can ship in any patch or minor; left
  out of `v1.1.0` to keep it free of public API changes.

- [x] **No `CLAUDE.md`/`api.md` in this repo.** `master` had both as canonical specs for
  `gogdl_flutter`, and `lumen-cli` has its own `CLAUDE.md`. Rustdoc (enforced by
  `#![warn(missing_docs)]`) covers per-item semantics, but there's no single tracker of which
  capabilities exist. That gap keeps widening: cloud saves went from list-only to a full
  download/upload surface with its own path-mapping rules in one release cycle.

  Fixed in `v1.1.5`: `CLAUDE.md` lists every `GogDl` method by area, with the gates and the release
  steps. There is no `api.md`; rustdoc stays the per-item spec.

## Low — style / clippy

- [x] **`Auth::is_valid`'s 60s margin had the wrong sign, and `SavesAuth::is_valid` copied it.**
  Fixed in `v1.1.2`: both counted a token as usable for 60 seconds *past* expiry (`t > now - 60`),
  against their doc comments. All three `is_valid`s (`Auth`, `SavesAuth`, `CdnUrlParams`) now go
  through the crate-private `Expiring` trait (`t - 60 > now`), so tokens are refreshed up to a minute earlier.
  Boundary tests for all three.

- [x] **Five fields are never read, and they are the crate's only rustc warnings.** Fixed in `v1.1.0`:
  `ProtonManagerInner`, `RemoteConfig.version`, `OsConfig.macos`, `OsConfigDetails.overlay` and
  `OverlayDetail` are deleted; `cargo build` has no warnings.
  - `ProtonManagerInner {}` (`src/proton/proton_manager.rs:20,26`) is a documented placeholder for
    caching that doesn't exist. Delete it until there's something to cache.
  - `RemoteConfig.version`, `OsConfig.macos`, `OsConfigDetails.overlay` and `OverlayDetail.supported`
    (`src/saves/remote_config.rs:20,115,123,134`) are deserialized "for completeness; unused".
    Serde ignores unknown keys, so dropping them costs nothing. Otherwise mark them
    `#[allow(dead_code)]` so real warnings stand out.

- [ ] **Save transfers do their (de)compression synchronously on the async runtime.** The downloader
  runs `GzDecoder::read_to_end` (`src/saves/saves_downloader.rs:160`) and the uploader runs
  `GzEncoder::write_all`/`finish` (`saves_uploader.rs:84-86`) inline in the task, over whole
  files held in memory. That is harmless for typical save sizes, but it's the pattern `31809e9`
  removed from the chunk downloader. `spawn_blocking`, or `async-compression` (already a
  dependency), would match.

- [ ] **A save response with no `ETag` is accepted unchecked, and `SaveFile.hash` can't serve as a
  fallback as documented.** `saves_downloader.rs:126-141` skips verification when the header is
  absent, although the listing's `hash` for the same object is in hand. `SaveFile.hash`'s doc says
  it is "for comparing against a local copy" (`save_files.rs:19-20`). But it is the MD5 of the
  stored *gzipped* bytes, as the downloader's ETag check shows, so it can't be compared to a local
  file without reproducing GOG's exact gzip stream. Correct the doc, and verify against `hash` when
  there's no ETag.

- [ ] **`SaveFile::relative_path` is still public, although its own doc says it is wrong for some
  games.** It collapses every Cyberpunk slot onto `sav.dat` (`save_files.rs:34-40`) and nothing in
  the crate calls it. Mark it `#[deprecated]` in favor of `relative_path_in`, or remove it.

- [ ] **`a813975` ("Expose Save Location APIs and path sanitizers") exposes nothing outside the
  crate.** It made `PathResolver`, `sanitize_filename` and `sanitize_relative_path` `pub`
  (`src/fs/mod.rs:5`) and loosened `save_location`/`save_files` items. But `mod fs` and
  `mod save_location` are private and `lib.rs` re-exports none of them, so the change is crate-wide
  visibility only. If a consumer (for example, `lumen-cli` sanitizing a Proton tag the same way) was
  meant to use them, they still can't. Otherwise the commit title misdescribes it.

- [ ] **The download hot path allocates and locks once per network read.** To share `stream_chunk`
  with the Proton downloader, `f4d7388` changed its callback from `AsyncFnMut(Bytes)` to
  `FnMut(Bytes) -> BoxFuture`, which can't hold a `&mut` to the decoder across the returned future.
  The decoder therefore lives in a `tokio::sync::Mutex` (`engine.rs:276,307-313`), and every read
  `Box::pin`s a future that awaits an always-uncontended lock (`proton_downloader.rs:125-133`
  repeats it with the duplex writer). The cost is small next to zlib and MD5, but it's paid
  thousands of times per second. A generic `AsyncWrite` sink parameter on `stream_chunk`, instead of
  a callback, would remove both the box and the lock.

- [ ] **`.map(|unit| unit.clone().unwrap().clone())` clones every surviving item twice.**
  `engine.rs:457,546,631`, right after taking the `Vec` by value. For `DepotFile` each clone
  copies a `Vec<Chunk>`. `.into_iter().flatten().collect()` does the filter and the unwrap with
  zero clones.

- [ ] **`HttpClient::stream_chunk` is a misnamed generic "GET a URL and stream it", and
  `2457bad` copied it instead of extending it.** `stream_chunk` (`http.rs:79-110`) has nothing
  chunk-specific and also carries multi-hundred-MB Proton tarballs. `stream_chunk_with_headers`
  (`:114-152`) is the same body plus a header loop and a `headers().clone()`. The three `get_json*`/
  `get_and_decode` helpers (`:242-334`) likewise repeat one send/status-check/read block. One
  `stream_url(url, headers) -> HeaderMap` would replace the first pair, and a shared
  `send_checked(request)` would replace the status-check block in all six, including `put_stream`.

- [ ] **The GitHub request headers are duplicated.** `Accept`/`User-Agent`/`X-GitHub-Api-Version`
  appear verbatim in `proton_ge_release.rs:77-81` and `proton_ge_releases_page.rs:39-43`. Make them a
  shared `const`.

- [ ] **Vestigial `let _ = body;` in `HttpClient::fetch`.** `http.rs:53` discards a `body` that
  `:59` then returns as `body: body`.

- [x] **The free-space check discards its own error detail, in three places.** Fixed on this branch
  for `v1.2.0`: the errors carry `required`/`available` and the path.
  `Downloader::download` (`engine.rs:177`), `repair` (`:80`) and
  `ProtonDownloader::download_proton_release` (`proton_downloader.rs:106`) each map
  `get_free_space()`'s `Err(_)` to a unit `CouldNotResolveFreeSpace`. That throws away the
  `FileSystemError::NoDiskMatchingPath(PathBuf)` that names the path (`src/fs/path_resolver.rs:54`).

- [ ] **`rand` with default features is a heavy dependency for retry jitter and upload request
  IDs.** `rand = "0.10.2"` pulls `chacha20`/`cpufeatures`/`getrandom`, which is more than a sleep
  duration and `saves_uploader.rs:92`'s `_gog_request_id` need. `tar` brings `filetime` and `xattr`
  (with `rustix`/`linux-raw-sys`) by default. The crate never reads xattrs, so
  `default-features = false` is worth checking there too.

- [x] **`DownloadEvent::Preparing`/`Prepared` are emitted back-to-back with no work between them.**
  `engine.rs:253-255`. Collapse them, or do real work in between.

  Deprecated in `v1.2.3`, removed in `v1.4.0`: both are `#[deprecated]` and still sent, followed by
  `DownloadEvent::Started`.

- [x] **`v1.0.1`'s tag doesn't match its `Cargo.toml`, and `main` is ahead of the newest tag.**
  `v1.0.1` (`8682797`) was tagged with `version = "1.0.0"` still in `Cargo.toml`, so a `Cargo.lock`
  resolved from it records the wrong version. `v1.0.2`–`v1.0.10` match. HEAD is two commits past
  `v1.0.10` with `Cargo.toml` still at `1.0.10`, so tagging it without a bump would repeat the
  `v1.0.1` mistake. Those two commits change what `get_owned_games` returns without changing its
  signature, so the next tag's notes should say so. The `-restart` tags (through `v0.1.12-restart`, plus the two `-debug` tags) also remain
  alongside the `v1.0.x` ones.
  Fixed in `v1.1.0`: `rust-toolchain.toml` is pinned, and the `release` job's
  `tool/release_notes.sh` fails a tag whose version disagrees with `Cargo.toml` or `Cargo.lock`.
  The old `v1.0.1` tag itself stays as it is.

**Clippy: 42 warnings, all fixed in `v1.1.0`** (`cargo clippy --all-targets -- -D warnings` is clean on `1.98.1`) (plus the six rustc warnings above, 48 total). 38 are auto-fixable.
Locations re-derived against `6b4b7f3` (owned-games/products against `e032b7d`):

- [x] **6 `unnecessary_cast`.** `backoff(attempt as u32)` on a `u32` at `http.rs:67` and
  `engine.rs:283,295,320,333,346`. The two verification arms (`:364`, `:378`) write bare
  `backoff(attempt)`.
- [x] 7 `let_and_return`: `download_manager.rs:70,85,100`, `engine.rs:459,548,633`,
  `product_bundle.rs:64`.
- [x] 5 module inception: `client::auth::auth`, `client::client`, `downloader::downloader`,
  `gogdl::gogdl`, `secure_links::secure_links`.
- [x] 4 `needless_return`: `http.rs:236,239`, `engine.rs:621`, `fs/path_resolver.rs:54`.
- [x] 3 `len_zero`: `engine.rs:101,121,198`.
- [x] 2 `redundant_field_names`: `http.rs:59` (`body: body`), `depot_info.rs:58`.
- [x] 2 `needless_borrow` on `refresh_auth(&self)`: `http.rs:56,63`.
- [x] 2 `manual_map`: `auth_manager.rs:113`, `depot_info.rs:39`.
- [x] 2 `or_insert_with(Vec::new)` → `or_default()`: `downloadable_product.rs:62`,
  `product_bundle.rs:59`.
- [x] 2 `map_or(false, ..)` → `is_some_and(..)`: `client/auth/credentials.rs:69` and its copy at
  `saves/saves_auth.rs:109`.
- [x] 2 redundant `&` in `format!`: `depot/depot_info.rs:80`, `saves/save_files.rs:152`.
- [x] 1 `collapsible_if`: `saves/saves_auth.rs:59`, the cache lookup's nested
  `if let .. { if auth.is_valid() }`. `links_manager.rs` already uses the let-chain form.
- [x] 1 `manual_filter_map`: `.filter(|d| d.is_ok()).map(|d| d.as_ref().unwrap())`
  (`games/owned_games.rs:48-49`). `filter_map(|d| d.as_ref().ok())` does the same with no `unwrap`.
  The fix for the silent-drop item above will replace it anyway.
- [x] Useless `format!` on a constant URL (`games/owned_products.rs:39`).
- [x] `OwnedProducts::default()` is an inherent method shadowing `std::default::Default`
  (`games/owned_products.rs:22`). Clippy's `should_implement_trait` stopped counting it once the
  type became crate-internal in `d6dbdc5`, but `#[derive(Default)]` is still the fix.
- [x] `GogdbDetails::produt_type` is misspelled, and it's mapped with `#[serde(alias = "type")]`
  where `rename` is meant (`games/owned_games.rs:19-21`). It works only because nothing serializes
  the struct.
- [x] Unused import `OwnedProducts` (`gogdl/facade.rs:11`), a rustc warning, not clippy.
- [x] One-offs: explicit closure for cloning (`depot/build_metadata.rs:40`), `io_other_error`
  (`downloader/util/hash.rs:44`).

  Module inception was fixed by renaming the inner modules (all private): `client::client` →
  `client::http`, `client::auth::auth` → `client::auth::credentials`, `downloader::downloader` →
  `downloader::engine`, `gogdl::gogdl` → `gogdl::facade`, `secure_links::secure_links` →
  `secure_links::links`. File references below use the new names.

The ~1,500 lines of new saves code added no clippy warnings beyond the three carried over from
`e4596b6`.

**Clippy still catches none of this crate's real defects.** Of the findings above (a
truncate-before-write on save files, tokens in error strings, a mutex held across network I/O, a
401 loop with no `continue`, an inverted expiry margin copied into a second type), clippy flags only
the inverted margin's `map_or` spelling, not its direction.

---

## Closed

One line per fixed item, newest first within each group. Detail is in the referenced commits.

### Foundation (`v1.1.0`)

- [x] **`download_files`, `backoff`, `is_valid`, the `Auth` round-trip and the captured-response
  deserializations (`SaveFile`, `RemoteConfig`, Proton releases, `gamesdb` game/DLC/collection) had
  no tests** — `src/downloader/engine.rs`, `src/test_support.rs`, `tests/`. The five `download_files`
  cases are pinned, plus a sixth for `attempt < MAX_ATTEMPTS - 4`. The `is_valid` tests were written
  against the wrong-sign margin and flipped in `v1.1.2`.

### Cloud saves, Proton & coverage

*From this pass, between `e4596b6` and `6b4b7f3`.*

- [x] **Cloud saves were list-only: no download or upload** — `2457bad`, `a458a6b`, `abfdda3`.
  `download_save_files`/`upload_save_files` now exist, with save-location expansion inside a Wine
  prefix. The new gaps they bring are open under [Medium — cloud saves](#medium--cloud-saves).
- [x] **A game whose location is named `saves` had every slot collapse onto one file** — `a458a6b`.
  `SaveFile::relative_path_in` drops a segment only when it is a declared location. It is pinned by
  `relative_path_in_keeps_every_slot_of_a_game_whose_location_is_saves`. The old
  `relative_path` is still public (open under Low).
- [x] **A cloud save name or location expression could write outside its directory** — `2457bad`,
  `abfdda3`. File names go through `PathResolver`. Location suffixes go through
  `sanitize_relative_path`, which drops `..` components, and a hostile `$USER` is rejected. Pinned by
  `parent_components_cannot_escape_the_variable_directory` and
  `a_hostile_host_user_name_cannot_leave_drive_c_users`.
- [x] **Frontends couldn't find an extracted Proton tree whose root carried an architecture
  suffix** — `26ea6c4`. The root is renamed to the sanitized tag and the path returned. Pinned by
  `architecture_suffixed_root_is_still_detected`. The same-name overlay it left behind is open under
  [Medium — Proton](#medium--proton).
- [x] **No automated tests at all** — narrowed, not closed. `2457bad` onward added the crate's first
  39 unit tests. The network-facing paths are still untested (open under
  [missing coverage](#medium--missing-coverage)).
- [x] **`lumen-cli`'s pinned tag was reachable only from `feature/saves`** — `feature/saves` is
  merged into `main`, and every `v1.0.x` tag, including `lumen-cli`'s current `v1.0.8`, is on
  `main`.

### Downloader reliability

*Everything above the next italic note is from the `6b4b7f3` pass, on top of `a813975`.*

- [x] **The batch couldn't short-circuit; a `ChunkHashMismatch` on one unit didn't stop the rest
  from spending their retry budgets.** `download_files` now drives units through
  `TryStreamExt::try_for_each_concurrent` instead of `buffer_unordered(..).collect()` followed by a
  fold: the first terminal error returns immediately, cancelling in-flight units and never starting
  ones not yet scheduled. A new `ProgressGuard` (`src/downloader/util/progress_guard.rs`) ties each
  attempt's `ProgressRegression` to `Drop` rather than eight hand-written send sites, so a unit
  cancelled mid-transfer still takes back what it reported — closing the Low item about exits that
  skipped `ProgressRegression`, and the `chunk_lenght` misspelling along with it. Documented on
  `DownloadError::ChunkHashMismatch`, `DownloadEvent::ProgressRegression`, `DownloadStageEvent` and
  `GogDl::download_game`/`repair_game`. Still untested — no test pins the short-circuit or the
  cancellation-regression pairing.
- [x] **Four exits from the attempt body didn't emit `ProgressRegression`** — closed by the same
  `ProgressGuard` change: `open_file`'s `?`, `OffsetWriter::new`'s `?`, and the two secure-link
  `return Err`s now all drop the guard at zero bytes reported, so the invariant ("every exit cancels
  what it reported") holds structurally instead of by each site remembering to send it.
- [x] **`chunk_lenght` was misspelled** — gone with the same change; the callback now calls
  `progress.report(chunk.len())` instead of naming a local.
- [x] **`DownloadEvent::Progress` is emitted once per network read** — resolved as intended
  behavior, not a bug. Coalescing to a display rate is explicitly the consumer's job, so a frontend
  can pick its own refresh rate rather than inherit one baked into the library; this crate reports
  at source granularity and will not rate-limit on the consumer's behalf. Now documented on
  `DownloadEvent::Progress`, `ProtonDownloadEvent::Progress` and `lib.rs`'s "Long-running
  operations" section. The two-hop unbounded-channel forwarding this item's write-up bundled in
  (`download`/`repair` re-sending `download_files`' events into the caller's channel) is unrelated
  to the emission rate and is not addressed by this; `lib.rs` already documents draining the
  receiver concurrently to avoid unbounded pile-up.

*Everything above the next italic note landed after `v0.0.13-restart`, between `66105ed` and
`e4596b6`.*

- [x] **`ChunkHashMismatch()` carried no detail, and its sibling arm's message had a dead
  `"ok"/"mismatch"` branch** — `c8170e2`. Now `ChunkHashMismatch { path, offset, expected, actual }`
  (`src/downloader/error.rs`), formatted in full. The short-response arm's message prints the actual
  digest instead of the always-"ok" conditional (`engine.rs:394-398`). Unverified: no test
  asserts the fields.
- [x] **Secure-link failures during a download were flattened to `ClientError::SecureLinksError {
  inner: String }`** — `004e68e`. Secure links are now resolved inside `download_files`' attempt
  loop and fail as `DownloadError::SecureLinksError(SecureLinksError)`. The stringly variant is
  deleted from `ClientError`. That the structured error is still retried uniformly, even for
  deterministic variants, is open above.
- [x] **`HttpClient::stream_chunk` owned secure-link resolution, coupling the client layer to
  `SecureLinksManager` and `FileType`** — `004e68e`. `stream_chunk` now takes a URL. `http.rs`
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
  hadn't absorbed** — resolved by `lumen-cli`'s pin bumps, now at `v1.0.8`.

### Auth & client

- [x] **Four authenticated endpoints fetched with no auth token at all** — `22bf182`.
- [x] **`Auth` and `TokenObserver` were dropped from the crate root by `9cd8d0a`** — `2ae200d` +
  `3ed4dd0`.
- [x] **`TokenObserver` couldn't be implemented outside the crate** — `32e8786`, re-broken by
  `9cd8d0a`, re-fixed by `2ae200d` + `3ed4dd0`.
- [x] **Error detail was invisible to consumers** — `5b7ca2f` re-exports every per-layer error
  enum. `ProtonError` and `SavesError` are re-exported too.
- [x] **Local expiry was a hard failure, not a refresh-and-retry case** — `5b7ca2f`. Not applied to the saves
  methods (open above).
- [x] **Any non-401 HTTP error was silently retried three times with its detail discarded** —
  `5b7ca2f`. The 401-without-auth loop was fixed in `v1.1.3`.
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
  `master`'s released `v0.1.1`** — `92030cf` moved to `1.0.0`, and `v1.0.2`–`v1.0.9` each match
  their tag. `lumen-cli`'s `Cargo.lock` records `1.0.8` for `tag=v1.0.8`. The `v1.0.1`
  exception is open under Low.
- [x] **Leftover debug `println!` in a filter closure** — `1d98eb7`.
  `grep -rn 'println!\|eprintln!\|log::\|tracing::' src/` is still empty. The Proton work
  (`b9a3e88`, "Log Proton GE release page fetches") left no logging in the tree.
- [x] **Redundant `if let None = ..` instead of `.is_none()` in five files** — `a962ef7`.
- [x] **`enum_variant_names` on `FileSystemError`** — no longer fires since `2100296`.
