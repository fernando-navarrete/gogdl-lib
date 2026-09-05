# GAPS.md

Open findings for `gogdl-lib` on the `restart` branch. Current tree: HEAD **`37aac65`** ("Refresh
GAPS documentation for current tree") **plus an uncommitted working tree** — modified `Cargo.toml`/
`Cargo.lock` (new `rand = "0.10.2"` dependency), `src/client/client.rs`,
`src/downloader/downloader.rs`, `src/downloader/mod.rs`, `src/downloader/util/mod.rs`, and a new
untracked `src/downloader/util/backoff.rs`. Two commits past the tip tag `v0.0.12-restart`.

That working tree adds a jittered exponential-backoff helper and wires it through both retry loops,
raises the attempt count from 3 to 6 behind a per-file `const MAX_ATTEMPTS` that the last-attempt
sentinels now derive from, retries a chunk that fails its MD5/length check (invalidating its secure
links and backing off first) instead of failing the whole download, and lets a CDN 401 retry
immediately. Against the previous pass it closes six items — three of them defects that pass's own
diff had introduced — and leaves three residuals: the cost of six attempts on a deterministic MD5
failure, `MAX_ATTEMPTS` being two constants with a backoff ceiling tied to neither, and five
redundant casts. The attempt-count rise is the one change that makes an existing item *worse*.

Line references were re-derived against this working tree, which shifted `downloader.rs` by 2-4
lines from the previous pass's numbers; refs here supersede any earlier wording. Fixed items are
collapsed to one line each in [Closed](#closed) at the bottom — the detail lives in the referenced
commits, or in the uncommitted diff for the six newest.

**Three standing facts that apply to almost every item here:**

- **No automated tests exist anywhere** (`grep -rn '#\[test\]\|#\[tokio::test\]' src/` is empty).
  Treat every fix in this doc as unverified. This is the direct reason the "a failed chunk is
  reported as success" bug was introduced twice (`f3a944a`, `ac061e0`) and fixed twice with nothing
  but a re-read catching it either time.
- **`v0.0.12-restart` as pushed still carries the retry off-by-one** fixed in `2469b13`. Anything
  resolving that tag gets a downloader that reports failed chunks as successful. Tag `2469b13`
  before any consumer bumps its pin.
- **Every fix newer than `2469b13` exists only in the working tree** — uncommitted, unpushed,
  untagged: the backoff helper, `MAX_ATTEMPTS`, the MD5-failure retry and the CDN-401 fast path.
  Six items in [Closed](#closed) point at that diff rather than at a commit.

`cargo build --lib` is free of rustc warnings. `cargo clippy --lib -- -W clippy::all` reports
**43 warnings**, up from 38: `attempt` is now `u32` (it comes from `0..MAX_ATTEMPTS`), so the five
surviving `backoff(attempt as u32)` calls are same-type casts. See the
[clippy](#low--style--clippy) section.

---

## High — downloader reliability

- [ ] **The MD5/length retry is now correct in shape but uncapped in cost: a chunk that will never
  verify is downloaded six times.** `downloader.rs:338-343` handles
  `writer.remaining() != 0 || actual_md5 != download_unit.md5` the same way as a transport failure —
  invalidate the secure links, `backoff(attempt)`, `continue` — which is the right treatment for the
  likely cause (a truncating or poisoned edge-cache entry) and is safe because `82c7980`/`f3a944a`
  rebuild the whole writer stack per attempt. But it is the one retrying arm where the failure is
  frequently *deterministic*, and `MAX_ATTEMPTS` just went from 3 to 6:
  - *A stale manifest costs 6× the transfer.* A size-correct MD5 mismatch against a bad manifest
    entry cannot be fixed by retrying, and the chunk is now re-downloaded in full five more times,
    with ~7.75s of backoff on top, before the identical error surfaces. It multiplies across every
    affected chunk, and nothing short-circuits: `download_files` drains the whole
    `buffer_unordered` stream and only then collects the results (`:360-364`), so the first chunk's
    six failed attempts don't stop the remaining units from spending theirs.
  - *Progress over-reports by up to 6× the chunk size.* This path only triggers *after* a chunk has
    streamed to completion, so every retry double-counts a whole chunk — see the `Progress` item
    below.
  - Splitting the two conditions is the cheap fix: `remaining() != 0` (a short response — always
    transport-shaped) keeps all six attempts; a size-correct MD5 mismatch retries once or twice and
    then fails, since the second identical answer from a re-fetched link is strong evidence the
    manifest, not the transfer, is wrong.

- [ ] **`DownloadEvent::Progress` is emitted once per network read — the exact event-rate problem
  mainline `v0.1.1` was cut to fix, now on two code paths.** `download_files`'s callback sends
  `DownloadEvent::Progress(chunk.len())` for every `Bytes` the `bytes_stream()` yields
  (`src/downloader/downloader.rs:281`), into an unbounded channel that a forwarding task re-sends
  into another unbounded channel (`:213-219` for `download`, `:132-138` for `repair`). With
  `self.threads` chunks in flight on a fast connection that is thousands of sends per second, bounded
  by read syscall size rather than by throughput.

  The workspace `CLAUDE.md` documents this same shape as the repair-download memory leak that
  `master`'s `v0.1.1` exists to fix: there, `gogdl_flutter`'s drain loops flooded an unbounded
  `StreamSink` faster than Dart could drain it, and the fix had two halves — coalescing in the bridge
  *and* cutting the rate at the source by moving `Progress` to the write-buffer flush boundary
  (`master:src/downloader/stream.rs:45-70` spells out the reasoning). The `restart` branch has
  neither half. Fix before any Flutter consumer is wired up to `repair_game`.

  Secondary: bytes streamed by an attempt that later fails are still counted, so progress
  over-reports by exactly the amount transferred before each retry. The MD5-failure retry
  (`:338-343`) makes this worse in the one case where it is guaranteed rather than incidental: that
  path only triggers *after* a chunk streamed to completion, so each such retry over-counts by a
  full chunk, and with `MAX_ATTEMPTS` now 6 a chunk that never verifies reports **6× its size**
  before erroring. Whichever fix lands must also reset or discount the progress reported for a
  failed attempt.

- [ ] **Secure links are never proactively expired.** `SecureLinksManager::links_cache`
  (`src/secure_links/links_manager.rs:19,32-63`) never evicts an entry on its own, and the fetched
  `CdnUrlParams`'s `expires_at`/`ttl` fields (`src/secure_links/secure_links.rs:11,13`) are parsed
  but never consulted. The reactive half works, and is now cheap — a CDN 401 triggers
  `invalidate_secure_links` and an immediate retry with no backoff
  (`src/downloader/downloader.rs:302-313`) — but every chunk request still has to *fail* once before
  a stale link gets replaced, and when several chunks hit that 401 together the misses aren't
  collapsed (see the thundering-herd item below). Wants the same shape `Auth::is_valid()`'s
  60s margin already has for tokens.

## Medium — duplication & consistency

- [ ] **`MAX_ATTEMPTS` fixed the sentinel coupling, but the constant itself is now declared twice
  and the backoff ceiling still isn't tied to it.** The bug class is gone: both loops read
  `for attempt in 0..MAX_ATTEMPTS` (`src/downloader/downloader.rs:265`, `src/client/client.rs:48`)
  and all six last-attempt sentinels are `attempt != MAX_ATTEMPTS - 1` (`downloader.rs:288,296,307,
  323,339`, `client.rs:65`), so changing the bound can no longer silently resurrect the
  swallowed-error regression that `ac061e0` shipped and `2469b13` fixed. Three residuals:
  - *Two constants, not one.* `const MAX_ATTEMPTS: u32 = 6;` is declared separately in
    `downloader.rs:29` and `client.rs:9`, both private to their file. They agree today by
    coincidence; nothing makes a change to one reach the other, and a reader has no way to tell
    whether the two loops are meant to share a policy or deliberately differ. One crate-level
    constant (or a `retry` module owning it alongside `backoff`) settles it.
  - *`backoff`'s ceiling is still an unrelated literal, and still never binds.*
    `src/downloader/util/backoff.rs:4-6` caps at 20s, but `backoff` is only called when
    `attempt != MAX_ATTEMPTS - 1`, so the largest `attempt` it ever sees is 4 and the largest
    ceiling actually reachable is `500ms * 2^4` = 8s. The cap first binds at `attempt == 6`, so it
    is dead for any `MAX_ATTEMPTS <= 7`. Either derive the cap from `MAX_ATTEMPTS` or drop it and
    say so.
  - *The delay budget moved a lot, silently.* Five backoffs with ceilings 0.5/1/2/4/8s is ~7.75s of
    sleeping on average and 15.5s worst case per chunk, on top of six connection attempts — where
    the previous tree spent ~0.75s over two. That is a better failure profile, but it is now the
    de-facto timeout for a chunk against a dead CDN and is worth stating as such rather than
    leaving it implied by two constants in two files.

- [ ] **The two retry loops are hand-copied rather than shared, and `download_files` carries a dead
  arm.** `download_files`'s match and `fetch`'s both independently carry an `HttpError{status, body}`
  arm and their own notion of what to retry; a change to one won't propagate to the other. And
  `download_files`'s `Err(ClientError::AuthError(err))` arm (`src/downloader/downloader.rs:286-293`)
  is unreachable: `stream_chunk` converts an auth failure during the secure-links fetch to
  `ClientError::SecureLinksError { inner: String }`, and `stream_chunk_inner` never calls
  `get_auth()` at all (CDN URLs are pre-signed, no bearer token), so a bare `AuthError` can never
  arrive here. It was copied from `fetch` and widened from `AuthError::TokenExpired` to any
  `AuthError`, so it now reads as if it handles more than it does — and each pass adds to it:
  `2469b13` gave it a `return Err(..)` and a `sleep`, the working tree swapped that `sleep` for a
  `backoff` call and re-derived its sentinel from `MAX_ATTEMPTS`, all of it on a path that cannot
  execute.

  The `HttpError` arm has picked up a smaller version of the same problem: `status ==
  reqwest::StatusCode::UNAUTHORIZED` is now tested twice within it (`:304` to invalidate, `:308` to
  skip the backoff), with the retry `continue` written out in both branches (`:309`, `:312`). One
  test setting a `delay: bool`, or hoisting the check above the `if attempt != MAX_ATTEMPTS - 1`,
  says the same thing once. The MD5 arm (`:339-343`) then writes the invalidate-and-back-off pair a
  third time — four arms and a verification check now share one recovery shape, spelled out
  separately at each site.

- [ ] **`Downloader::repair` is a near-verbatim copy of `Downloader::download`.** `repair`
  (`src/downloader/downloader.rs:43-142`) and `download` (`:143-223`) are the same function apart
  from one inserted stage. Lines `49-105` of `repair` and `149-205` of `download` — path-resolver
  construction, the `depot_files` flat_map, the whole file-size-verification stage with its
  channel/`tokio::join!`/forwarding boilerplate, the free-space computation and its two error
  returns, the whole allocation stage with the same boilerplate again, and the `FileAllocationError`
  abort — are identical modulo two extra `drop()`s and the stage-forwarding variable names. The only
  real difference is `repair`'s `verify_download_units` stage (`:107-125`), whose `missing_units` it
  passes to `download_files` where `download` passes `DownloadUnit::from_product_bundles(&bundles)`
  wholesale.

  Every fix in this doc against `download`'s pipeline has to be applied twice, and the free-space
  error-discard item below already exists in two places because of it. `download` is expressible as
  `repair` with the verification stage skipped — or better, as a shared private helper taking a
  "which units to download" closure.

- [ ] **`download` re-downloads every chunk regardless of what's already correct on disk, so `repair`
  is the crate's only resume path — and nothing says so.** `download` runs a file-size verification
  stage and computes `missing_files`, then ignores that result when building the transfer list:
  `DownloadUnit::from_product_bundles(&bundles)` (`src/downloader/downloader.rs:209`) enumerates
  every chunk of every file in every bundle, including files that just verified as complete. An
  interrupted download restarted through `download_game` re-transfers the whole game from byte 0,
  while `repair_game` transfers only what fails MD5.

  Given pause/resume was deliberately removed chain-wide (see the workspace `CLAUDE.md`),
  `repair_game` is the de-facto resume entry point — and a consumer has no way to know that from the
  API surface: the two methods have identical signatures and neither has a doc comment. Either make
  `download` skip already-verified units, or document that resuming means calling `repair_game`.

- [ ] **`repair` checksums the chunks of files it has just allocated.** Stage 2 allocates every file
  that failed size verification (`set_len` on a fresh or truncated file,
  `src/downloader/fs/path_resolver.rs:76`), and stage 3 then MD5s **every** unit of **every** file
  (`src/downloader/downloader.rs:107-119`) — including those just-allocated, all-zero ranges, whose
  chunks cannot possibly match. On a repair where a large file is missing outright that's a full-size
  read plus MD5 of zeroes purely to conclude what stage 1 already knew. Filter the units belonging to
  `missing_files` out of the verification pass and add them straight to the download list.

- [ ] **`DownloadStageEvent::VerificationStage` is a source-breaking addition for existing
  consumers.** The variant (`src/downloader/progress_reporting/download_stage_event.rs:9`) is emitted
  only by `repair`'s stage 3, but it widens a `pub` enum that consumers match exhaustively.
  `lumen-cli` (sibling repo, currently pinned at `tag = "v0.0.9-restart"` in its `Cargo.toml:12`) has
  an exhaustive four-arm `match ev` with no `_` arm in `apply_download_stage_event`
  (`src/middleware/downloads.rs:555-607`), so bumping its pin is a hard `E0004` — on top of the
  `E0599: no method named 'get_secure_links'` break the export item below flags for the same bump.
  Its `DownloadStage` enum has no verification state to map the new variant onto either, so this
  needs a code change there, not just a new arm. Same applies to `gogdl_flutter`'s `restart` branch
  whenever `repair_game` gets bridged.

- [ ] **Secure-link fetches aren't collapsed across concurrent chunk downloads, and `a11273a`
  deleted the pre-fetch that used to hide it.** `SecureLinksManager::get_secure_links`/
  `invalidate_secure_links` (`src/secure_links/links_manager.rs:32-67`; cache at `:19`) have no
  in-flight-request dedup. `download_files` runs up to `self.threads` chunk downloads concurrently via
  `buffer_unordered` (`src/downloader/downloader.rs:258,360`), each calling `stream_chunk`
  independently, and `stream_chunk` calls `get_secure_links` per chunk (`src/client/client.rs:87`).
  - *Cold start (new with `a11273a`):* `download_files` used to warm the cache first — a
    `stream::iter(bundles).map(|b| get_secure_links(..)).buffer_unordered(self.threads)` block that
    `a11273a` removed along with the `(String, DownloadUnit)` tupling it sat beside. Nothing replaced
    it, so the first `self.threads` chunk tasks all miss the empty `links_cache` at once and each
    issues its own round-trip. The cache-check and the fetch are not under one lock
    (`links_manager.rs:39-62`), so the mutex doesn't collapse them either. **Per product bundle, on
    every download and every repair.**
  - *Expiry:* if a link expires with several chunks in flight, each hits the CDN 401 at roughly the
    same time, each calls `invalidate_secure_links`, and each re-fetches independently.

  Restoring the pre-fetch would paper over the cold-start half only; an in-flight dedup (a
  per-`game_id` `OnceCell`/shared future — the shape `PathResolver::dir_cache` already uses at
  `src/downloader/fs/path_resolver.rs:13,85-99`) fixes both.

- [ ] **Types a consumer must be able to name are not exported: `DepotFile`, `Chunk`,
  `DownloadUnit`.** The exported `ProductBundle` declares `pub product_files: Vec<DepotFile>`
  (`src/downloader/product_bundle.rs:8-10`); `DepotFile.chunks` (`src/depot/depot_info.rs:33`) is
  `Option<Vec<Chunk>>`; `DepotFile::to_download_units` (`:45`) is a `pub` method on the unexported
  `DepotFile` returning `Vec<DownloadUnit>`. None of the three is re-exported from `src/lib.rs`
  (`998829d` dropped the first two without narrowing what still *emits* them). A consumer that walks
  `ProductBundle.product_files` — which `verify_files`/`download_game` require holding onto, since
  `ProductBundle` isn't `Clone` — receives a `Vec` of a type it cannot name in a signature, a `let`
  binding, or a test fixture.

  `lumen-cli` previously failed with `E0432: unresolved imports gogdl_lib::DepotFile,
  gogdl_lib::SecureLinks` and has doc comments noting the workaround
  (`src/middleware/downloads.rs:176-182`). It also calls `gog.get_secure_links(product_id)` at two
  sites (`:131,200`) — a method `2ae200d` deleted outright — so once it bumps its pin those become a
  hard `E0599`, not a workaroundable type-naming problem. **This consumer needs either the method
  restored or a replacement API before that pin bump.** Either re-export `DepotFile`/`Chunk`/
  `DownloadUnit`, or narrow the public field that leaks them.

- [ ] **A dead refresh token and a routine expiry both surface as the same opaque, unstructured
  error** — effectively a recurrence of the `982dc82` collapsing regression (see `lumen-cli/CLAUDE.md`,
  "Auth refresh regression"). `login_with_code` (`src/client/auth/auth_manager.rs:50-58`) and
  `refresh_auth` (`:99-107`) each call `client.fetch_no_retry(&url, false, false)`, match
  `Err(ClientError::AuthError(e)) => return Err(e)` first, and otherwise fall to
  `AuthError::ClientError { inner: e.to_string() }`. But `require_auth` is hardcoded `false` in both
  calls, so `inner_fetch` (`src/client/client.rs:129-150`) can **never** produce
  `ClientError::AuthError` on these paths — that branch only fires when `require_auth: true` and the
  internal `get_auth()` fails locally.

  So every real failure from `AUTH_URL`/`REFRESH_URL` — including a 401 because the refresh token is
  dead rather than merely expired — falls to the stringified arm. `AuthError` is now `pub` from the
  crate root, so a consumer can at least distinguish `ClientError { inner }` from
  `TokenExpired`/`NotAuthenticated`, but `inner` is a `Display`-formatted string with no structure: a
  dead refresh token and a transient network blip remain indistinguishable *within* that variant.
  Fix needs a status-aware variant on `AuthError` for this case — or at minimum, delete the dead
  `Err(ClientError::AuthError(e))` arm so the code stops implying a distinction it can't make.

- [ ] **Five dead variants per error enum.** `Http`/`UrlParseError`/`NetworkError`/`DecodeError`/
  `DeflateError` on `DepotError`, `GamesError`, `SecureLinksError` and `DownloadError` are
  unconstructible. `43c1d85` deleted the four hand-written `impl From<ClientError> for XError` blocks
  that used to translate into them, replacing each with a blanket `ClientError(#[from] ClientError)`.
  Every network call in `depot/`, `games/` and `secure_links/` goes exclusively through `HttpClient`
  (`grep -rn "reqwest::\|url::Url::parse\|serde_json::from_str" src/depot src/games src/secure_links`
  matches nothing outside `error.rs`), and `downloader/` is the same but for its own directly
  constructed `DeflateError` (`src/downloader/downloader.rs:274,334,344`). Confirmed
  unconstructible with `grep -rn "::Http {" src/` and the per-variant equivalents, all empty.

  Because these enums are `pub`, `dead_code` doesn't warn, so clippy will never surface this. Either
  delete the unreachable variants (and their now-unused `use reqwest::StatusCode`/`use std::io`
  imports, e.g. `src/depot/error.rs:1,3`) or reinstate a translation that uses them. A working
  wrapper *plus* five dead siblings per enum is confusing surface for whoever reads these next.

- [ ] **"Not a game" handling is inconsistent across near-identical fetchers.** Only
  `GameDetails::get_game_details` (`src/games/game_details.rs:17-24`) turns a cached `None` into
  `GamesError::ProductNotAGame`. `GameBuilds`, `GameLinks`, `GameSummary` and `OwnedGames` don't, so
  calling e.g. `get_game_builds` for a DLC/non-game product surfaces a raw `DecodeError`.

- [ ] **Caching is inconsistent between depot data of similar shape.** `DepotManagerInner` caches
  `ProductDetails` (`src/depot/depot_manager.rs:20`) but has no cache for `DepotInfo` — the actual
  per-depot manifest, likely the largest payload the crate fetches.
  `ProductBundle::get_download_files` (`src/downloader/product_bundle.rs`) re-fetches every depot's
  manifest from the CDN on every call, even for a build/product combination
  `DownloadableProduct::get_downloadable_products` resolved seconds earlier — and *that* one is
  cached.

- [ ] **Language and OS are hardcoded with no selection surface.**
  `BuildMetadata::filter_languages` is always called with `"en-US"`
  (`src/depot/build_metadata.rs:34`), and `GameBuilds::get_game_builds` always queries
  `os/windows/builds` (`src/games/game_build.rs:34`). No way for a caller to ask for a different
  language depot or a native Linux build. The OS choice may well be intentional given Proton is used
  for everything — worth documenting as a deliberate limitation rather than leaving it implicit.

- [ ] **`GameScreenshots::resolve_links` picks a magic formatter index.**
  `src/games/game_screenshots.rs:76-77` calls `formatter.get(2)` — the third available formatter —
  with no explanation of why index 2, and silently drops the screenshot (`continue`) if fewer than 3
  formatters are present instead of falling back to whatever is available.

## Medium — missing coverage

- [ ] **No automated tests anywhere in the crate.** Given the `restart` branch's explicit goal of a
  careful, from-scratch rebuild (per the workspace `CLAUDE.md`), this is worth addressing before the
  crate grows further. Highest-value first:
  - **One `#[tokio::test]` driving `download_files` against a chunk source that fails every
    attempt.** This would have caught the swallowed-error regression on the day it was written, both
    times, and would fail today against `v0.0.12-restart`. The same harness, with a source that
    serves truncated or wrong-MD5 bytes on the first attempt and good bytes on the second, pins the
    working tree's new MD5-failure retry — including that it reports the right byte total.
  - **`backoff`'s bounds** (`src/downloader/util/backoff.rs`) — the ceiling is deterministic even
    though the draw isn't, so `attempt` → ceiling is a plain table test, and a single assertion that
    the ceiling at `MAX_ATTEMPTS - 2` is the cap would have caught the still-dead 20s `.min(..)`
    when the helper was written.
  - Auth logic, all unit-testable without network: `Auth::is_valid` boundaries (including the 60s
    buffer), the `to_string`/`from_string` restore round-trip (regressed once in `998829d`, fixed in
    `6c76f03`, still unpinned), `refresh_auth` persisting `valid_until` (broke twice across
    refactors), and concurrent `refresh_auth` collapsing.

- [ ] **No `CLAUDE.md`/`api.md` on the `restart` branch.** Both exist on `master` and are treated as
  canonical specs for consumers (`gogdl_flutter`); `restart` has neither, so there's no single
  reference tracking what public surface has been rebuilt vs. still stubbed. `a11273a` sharpens
  this: it adds a public `GogDl::repair_game` and a new `DownloadStageEvent` variant with no doc
  comment anywhere, and the download-vs-repair semantics (which resumes, which re-transfers, which
  stages each emits) are discoverable only by reading `downloader.rs`.

## Low — style / clippy

- [ ] **Vestigial `let _ = body;` in two retry arms.** `src/downloader/downloader.rs:303` discards
  `body` into `_` eleven lines before `:314` returns `ClientError::HttpError { status, body }` with
  it. The statement was load-bearing at `v0.0.11-restart` — it kept rustc quiet about the then-unused
  binding, and so is part of why the swallowed-error bug drew no warning — but now reads as if the
  value were deliberately dropped. `HttpClient::fetch` has the identical pair (`client.rs:52`
  discarding a `body` that `:58` returns). Delete both, and `:58`'s `body: body` with them.

- [ ] **The free-space check discards its own error detail, in two places.** `Downloader::download`
  (`src/downloader/downloader.rs:176-179`) and its copy in `Downloader::repair` (`:77-80`) each match
  `path_resolver.get_free_space()`'s `Err(_)` and always return the unit variant
  `DownloadError::CouldNotResolveFreeSpace`, throwing away the
  `FileSystemError::NoDiskMatchingPath(PathBuf)` that names exactly which resolved base path had no
  matching disk (`src/downloader/fs/path_resolver.rs:39-58`). Carry the path (or the whole
  `FileSystemError`) into the variant.

- [ ] **The retry primitives are scattered across layers.** `backoff` lives in `downloader::util`
  but `client.rs:17` imports it as `crate::downloader::{FileType, backoff}` while `downloader.rs:9`
  imports the same function as `crate::downloader::util::backoff` — two paths to one item, one of
  which makes the client layer depend on the downloader for a generic retry primitive.
  `downloader/mod.rs:23` re-exports it beside the public event types; `mod downloader` is private in
  `lib.rs`, so nothing leaks from the crate root. `MAX_ATTEMPTS` is then declared once per file
  (`client.rs:9`, `downloader.rs:29`), and `client.rs`'s copy sits *between* the `use std::..` line
  and the `use crate::{..}` block rather than with the other items. A small `retry` module owning
  `MAX_ATTEMPTS` and `backoff` together — with a doc comment stating the base, ceiling and jitter
  policy, which nothing currently records — puts all of it in one place.

- [ ] **`rand` with default features is a heavy dependency for retry jitter.** `rand = "0.10.2"`
  pulls `chacha20`, `getrandom 0.4`, `rand_core 0.10` and `r-efi` — and with `chacha20` comes
  `cpufeatures`, which `2100296` had removed from the tree along with `sha2`. A ChaCha20 CSPRNG for
  a sleep duration is more than the job needs: a non-cryptographic generator, or jitter derived
  from the chunk hash the task already holds, keeps the dependency graph where `2100296` left it.

**Clippy: 43 warnings** against this tree, up from 38. Locations re-derived:

- [ ] **5 `unnecessary_cast` — new with this working tree.** `attempt` is `u32` now that it comes
  from `0..MAX_ATTEMPTS`, so `backoff(attempt as u32)` casts `u32` to `u32` at `client.rs:66` and
  `downloader.rs:289,297,311,324`. The MD5 arm added last (`downloader.rs:341`) already writes the
  bare `backoff(attempt)`, so the five casts are also inconsistent with the sixth call site.
- [ ] 7 `let_and_return` — `download_manager.rs:70,85,100`, `downloader.rs:424,512,597`,
  `product_bundle.rs:47`.
- [ ] 4 "redundant redefinition of a binding `path_resolver`" — `downloader.rs:260,376,435,523`, all
  the same `let path_resolver = path_resolver;` idiom moving it into a closure.
- [ ] 5 module inception — `client::auth::auth`, `client::client`, `downloader::downloader`,
  `gogdl::gogdl`, `secure_links::secure_links`.
- [ ] 4 `needless_return` — `client.rs:144,147`, `downloader.rs:585`, `path_resolver.rs:49`.
- [ ] 3 `redundant_field_names` — `client.rs:58` (`body: body`), `depot_info.rs:58`
  (`offset: offset`), `downloader.rs:300` (`inner: inner`).
- [ ] 3 `len_zero` — `downloader.rs:101,122,200`.
- [ ] 2 `needless_borrow` on `refresh_auth(&self)` — `client.rs:55,62`.
- [ ] 2 `manual_map` — `auth_manager.rs:113` (clone-observer-out-of-guard, wants `.cloned()`),
  `depot_info.rs:39` (`DepotFile::size`, wants `.as_ref().map(..)`).
- [ ] 2 `or_insert_with(Vec::new)` → `or_default()` — `downloadable_product.rs:50`,
  `product_bundle.rs:42`.
- [ ] `Auth::is_valid`'s `map_or(false, ..)` should be `is_some_and(..)`
  (`client/auth/auth.rs:33-34`).
- [ ] `OwnedGames::default()` is a hand-written inherent method shadowing `std::default::Default`
  (`games/owned_games.rs:14-16`) — implement the trait.
- [ ] Useless `format!` with no interpolation on `format!("https://embed.gog.com/user/data/games")`
  (`games/owned_games.rs:25`).
- [ ] One-offs: explicit-closure-for-cloning (`depot/build_metadata.rs:38`), `io_other_error`
  (`util/hash.rs:44`), redundant `&` in a `format!` (`depot/depot_info.rs:80`).

`cargo clippy --fix --lib -p gogdl-lib -- -W clippy::all` auto-applies most of these — but only
against a clean tree; it will otherwise try to "fix" whatever is mid-edit.

**Clippy caught none of the four retry defects this branch has shipped.** `for attempt in 0..2`
against `attempt != 2` and `for attempt in 0..3` against `attempt != 2` produce byte-identical clippy
output; a `for` loop missing its `break` fires no lint; the `return`s an always-true guard renders
dead are unreachable only at runtime, so `unreachable_code` never fires; and two consecutive
`sleep`s — the doubled delay `fetch` briefly carried — are perfectly legal code. What clippy *did*
notice about this retry code is five redundant casts. Only reading the code, or a test, tells the
real defects apart.

---

## Closed

One line per fixed item, newest first within each group. Detail is in the referenced commits.

### Downloader reliability

*The first six are in the **uncommitted working tree**, not in any commit or tag.*

- [x] **A chunk failing its MD5/length check was never retried, failing the whole download on the
  first bad byte** — working tree. `downloader.rs:338-343` invalidates the secure links, backs off
  and `continue`s. Its cost at `MAX_ATTEMPTS = 6` on a deterministic mismatch is open above.
- [x] **The MD5/length retry hit the same edge node immediately, with no delay and no link
  invalidation** — working tree, in the same pass that introduced it; `:340-341` now does both,
  matching the transport-shaped arms.
- [x] **The retry delay was linear and unjittered, so every in-flight chunk task retried in
  lockstep** — working tree. `src/downloader/util/backoff.rs` draws a full-jitter delay from
  `0..=min(500ms * 2^attempt, 20s)`; every retrying site uses it. The dead 20s `.min(..)` is open
  above.
- [x] **`HttpClient::fetch` slept twice per network retry** — working tree, in the pass that
  introduced it: `backoff(..)` had been *appended* to the linear `tokio::time::sleep` rather than
  replacing it, so a retried fetch waited 2s + jitter, then 4s + jitter. The `sleep` and the
  now-unused `time::Duration` import are both gone.
- [x] **The retry bound and its five last-attempt sentinels were independent literals, in two
  loops** — working tree. `const MAX_ATTEMPTS: u32 = 6;` with `attempt != MAX_ATTEMPTS - 1` at every
  sentinel; the `0..2`-vs-`!= 2` regression class can't recur. That the constant is declared once
  per file, and that `backoff`'s ceiling still isn't derived from it, are open above.
- [x] **A routine CDN 401 paid the same congestion backoff as a downed CDN** — working tree. The
  `HttpError` arm invalidates the secure links and retries immediately when
  `status == UNAUTHORIZED` (`:304-312`). The unreachable `AuthError` arm and the reachable
  `SecureLinksError` arm still back off; the former is dead code (tracked above), the latter covers
  a failed secure-links *fetch*, which is transport-shaped, so backing off there is defensible.
- [x] **Both retry loops off by one (`0..2` bound tested against `!= 2`), making every last-attempt
  branch dead code** — `2469b13`. Both bounds back to `0..3`; all five `return Err(..)` reachable.
  Made structurally impossible afterwards by `MAX_ATTEMPTS` above.
- [x] **A chunk that fails every attempt was reported as a successful download** — introduced by
  `f3a944a`, fixed in `ac061e0`, re-introduced by the same commit's `0..2` bound, closed by
  `2469b13`. Failures now propagate through `buffer_unordered` with full detail.
- [x] **The retry loop never broke, so every chunk was downloaded three times** — `ac061e0`. `break`
  at `:355`, after `decoder.shutdown()`/MD5 verification, so the successful attempt is still
  verified.
- [x] **Local, non-retryable failures burned all three attempts** — `7eb5d5e`.
  `ChunkStreamCallbackError` (full disk, over-length guard, zlib error) and `UrlParseError` return on
  first occurrence, above the catch-all, with no `attempt` guard.
- [x] **No retry/backoff for transient network failures** — `2b1cd7f` (arms + delay), `2469b13`
  (delay on all four `download_files` arms, linear `(attempt+1)*2`, none on the way out), then the
  working tree's jittered `backoff` helper replacing that delay and raising 3 attempts to 6.
- [x] **`stream_chunk`'s retry resumed into a dirty writer, so its `NetworkError` arm could never
  recover** — `82c7980` + `f3a944a` moved the retry loop up to `download_files`, which owns the
  writer stack and now rebuilds `open_file` → `OffsetWriter` → `HashingWriter` → `ZlibDecoder` per
  attempt.
- [x] **`stream_chunk` bypassed the `fetch`/`inner_fetch` funnel entirely** — resolved via its own
  retry path (`de52483`, `e4a8560`), then relocated to `download_files`. Residual duplication and the
  dead `AuthError` arm are tracked as open items above.
- [x] **Secure links cached forever with no expiry handling** — reactive half fixed in `de52483`
  (CDN 401 → `invalidate_secure_links` → re-fetch), relocated intact by `f3a944a`, which also
  invalidates on `SecureLinksError`. Proactive half is open above.
- [x] **Blocking syscalls ran directly inside async tasks** — `31809e9` + `46b6ce3`. Real async I/O
  rather than `spawn_blocking`: `tokio::fs`, `OffsetWriter` as `AsyncWrite`, `AsyncFnMut` callback.
  `grep -rn 'std::fs::\|write_at' src/downloader/` is empty.
- [x] **`Downloader::verify` discarded its own result** — `71d49a8`. Returns
  `DownloadError::ChunkIntegrityCheckFailed(n)` instead of unconditional `Ok(())`.
- [x] **One failed file allocation aborts the whole download** — resolved as *intended* behavior,
  confirmed with the person driving the rebuild. `2100296` reinforces it with a proactive free-space
  check and a specific `NotEnoughFreeSpace` error.
- [x] **Only per-chunk MD5 verified; SHA-256 support was unused dead code** — resolved by deletion in
  `2100296`. `DepotFile.sha256`, `ChecksumAlgorithm` and the `sha2`/`cpufeatures` deps are gone.

### Auth & client

- [x] **Four authenticated endpoints fetched with no auth token at all** — `22bf182`, correcting the
  `decode`/`require_auth` bools swapped at four call sites when `9cd8d0a` changed the signature.
- [x] **`Auth` and `TokenObserver` were dropped from the crate root by `9cd8d0a`'s
  `src/auth/*` → `src/client/auth/*` move** — `2ae200d` (`TokenObserver`) + `3ed4dd0` (`Auth`).
  Verified with an external `impl TokenObserver for Dummy`.
- [x] **`TokenObserver` couldn't be implemented outside the crate** — `32e8786`, re-broken by
  `9cd8d0a`, re-fixed by `2ae200d` + `3ed4dd0`. `AuthManager` itself stays crate-private.
- [x] **Error detail was invisible to consumers** — `5b7ca2f` re-exports every per-layer error enum
  (`AuthError`, `ClientError`, `DepotError`, `DownloadError`, `GamesError`, `SecureLinksError`) from
  `src/lib.rs`. No flat top-level `GogDlError::AuthError` — auth failures still arrive nested as
  `XError::ClientError(ClientError::AuthError(..))`.
- [x] **Local expiry was a hard failure, not a refresh-and-retry case** — `5b7ca2f`. `fetch` gained a
  dedicated `Err(ClientError::AuthError(AuthError::TokenExpired))` arm, so the common "just launched
  after an hour" case refreshes instead of failing.
- [x] **Any non-401 HTTP error was silently retried three times with its detail discarded** —
  `5b7ca2f`. Explicit `else` returns the original status/body immediately. A brief detour that also
  retried 403 was reverted before landing, correctly.
- [x] **`refresh_auth` persisted tokens without `valid_until`** — `998829d`, silently regressed by
  `9a1f780`, fixed again in `5b7ca2f`. Broke twice across refactors with no test; see the coverage
  item above.
- [x] **Restoring persisted tokens locked the app out entirely** — `6c76f03` dropped
  `#[serde(skip_deserializing)]` from `Auth::valid_until`.
- [x] **`valid_until` was dead** — `998829d` wired `Auth::is_valid()` and gated `get_auth` on it.
- [x] **`is_valid()` had no clock-skew / in-flight margin** — `d679048` subtracts a 60s buffer.
- [x] **Silent, internal token refreshes were invisible to callers** — `998829d` + `b87d8dd`; the
  observer hands over the whole `Auth`, so the rotated refresh token reaches the app.
- [x] **Token observer only emitted the access token** — `b87d8dd`.
- [x] **A registered `TokenObserver` could never be replaced with "none"** — `d679048` adds
  `remove_token_observer`, wired out through `HttpClient` and `GogDl`.
- [x] **The observer callback ran while the `inner` mutex was held** — `51b7a53` clones it out and
  drops the guard first.
- [x] **The `inner` mutex was held across the refresh/login network round-trip** — `9a1f780`.
- [x] **`refresh_lock` serialized refreshes but didn't collapse them** — fixed by snapshotting the
  access token before queueing and short-circuiting if it changed while waiting. Token-identity
  comparison rather than a post-lock `is_valid()` check, so a genuine 401 on a locally-valid token
  still reaches the network. One redundant refresh remains possible, rare and bounded.
- [x] **`AuthManager::set_auth` was unreachable** — resolved by removal.
- [x] **Twelve `get_auth().await.unwrap()` calls became reachable panics in `998829d`** — `9a1f780`.
  Every authed request now does exactly one `get_auth()` await, down from two.
- [x] **Two `.parse().unwrap()` calls could crash the process on untrusted input, and a sibling file
  handled the identical failure differently** — fixed on top of `5b7ca2f`.
  `SecureLinksManager::get_secure_links` returns `SecureLinksError::IncorrectGameId`; both filter
  closures (`downloadable_product.rs`, `product_bundle.rs`) now drop the entry the same way, so the
  panic-vs-`unwrap_or(0)` inconsistency is gone too.
- [x] **The "fetch → on 401 refresh → retry once" block was copy-pasted six times** — `9cd8d0a`
  folded auth into `HttpClient` and deleted all six pre-flight blocks. Introduced the swapped-bool
  and local-expiry regressions above along the way; both since fixed.
- [x] **`DownloadError` was left out of the `a962ef7` error unification** — `9a1f780`, restructured
  by `9cd8d0a`. Only `ClientError(#[from] ClientError)` remains.

### Style

- [x] **Leftover debug `println!` in a filter closure** — `1d98eb7`.
  `grep -rn 'println!\|eprintln!\|log::\|tracing::' src/` is empty crate-wide; the crate still has no
  logging facade at all.
- [x] **Redundant `if let None = ..` instead of `.is_none()` in five files** — `a962ef7`.
- [x] **`enum_variant_names` on `FileSystemError`** — no longer fires, as a side effect of
  `2100296`'s `NoDiskMatchingPath` variant breaking the uniform `Error` postfix. Never a real signal:
  `ClientError` escapes the same lint for the same reason.
