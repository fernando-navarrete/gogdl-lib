# GAPS.md

Open findings for `gogdl-lib` on the `restart` branch. Current tree: HEAD **`04ff377`** ("Report
progress regression on download retries", pushed — `origin/restart` is at the same commit) **plus an
uncommitted working tree** — modified `src/downloader/downloader.rs` only. Five commits past the tip
tag `v0.0.12-restart`.

The pass before last's uncommitted diff is now `770537c`; its items are re-labelled to that commit in
[Closed](#closed).

`04ff377` did three things: it sent `ProgressRegression(reported_bytes)` from the two
verification-failure retries and the two non-retryable client-error arms (emit sites four → eight),
gave `Progress`/`ProgressRegression` one-line doc comments, and cut the MD5 arm's attempt budget by
changing its sentinel from `attempt != MAX_ATTEMPTS - 1` to `attempt != MAX_ATTEMPTS - 3`. The first
two landed. The third was a regression — an equality test against a fixed *interior* index, so an
MD5 mismatch first seen on attempt 4 or 5 took the `continue` branch, fell out of the `for` loop, and
the chunk was reported as a **successful download**: the fourth appearance of "a failed chunk is
reported as success" on this branch.

**The working tree fixes it**, in two small changes:

1. The MD5 sentinel becomes `attempt < MAX_ATTEMPTS - 4` (`downloader.rs:348`). Because it is a
   comparison rather than an equality, every attempt at or past the bound returns
   `Err(ChunkHashMismatch())` — attempts 2, 3, 4 and 5 all take the `return`, so the loop cannot run
   off its end into `Ok(())` no matter which attempt the mismatch first appears on. The
   silent-success path is closed. Budget is now 3 attempts for a hash mismatch, 6 for everything
   else.
2. `decoder.shutdown()` stops being a bare `?` and becomes an
   `if let Err(err) = ..` that emits `ProgressRegression(reported_bytes)` before returning
   (`:338-341`) — the one remaining exit that could leak a full chunk's worth of reported bytes.
   Emit sites are now nine.

That leaves `open_file`'s `?` (`:268`) as the only exit from the attempt body that doesn't cancel its
reported bytes, and it runs before a single byte is reported, so its total is always 0 — demoted to
[Low](#low--style--clippy). What survives from the previous pass's reading is the *style* point, not
a correctness one: the MD5 bound is still derived from `MAX_ATTEMPTS` by subtracting a literal, now
`- 4` instead of `- 3` (see the [`MAX_ATTEMPTS` item](#medium--duplication--consistency)). The
failure mode if that constant ever moves is now "fewer retries", never "silent success", which is the
part that mattered.

Line references were re-derived against this working tree; `downloader.rs` shifted +1 below `:338`
from the previous pass's numbers. Refs here supersede any earlier wording. Fixed items are collapsed
to one line each in [Closed](#closed) at the bottom — the detail lives in the referenced commits, or
in the uncommitted diff for the two newest.

**Three standing facts that apply to almost every item here:**

- **No automated tests exist anywhere** (`grep -rn '#\[test\]\|#\[tokio::test\]' src/` is empty).
  Treat every fix in this doc as unverified. This is the direct reason the "a failed chunk is
  reported as success" bug was introduced twice (`f3a944a`, `ac061e0`) and fixed twice with nothing
  but a re-read catching it either time. It is why the fourth recurrence shipped in `04ff377` too —
  a test that fails a chunk's transport a few times and *then* serves wrong-MD5 bytes would have
  gone red the moment that commit was written — and it is why the working tree's fix is *also*
  unverified: nothing pins the new sentinel, so the fifth recurrence has the same clear run at it.
- **`v0.0.12-restart` as pushed still carries the retry off-by-one** fixed in `2469b13`. Anything
  resolving that tag gets a downloader that reports failed chunks as successful, and `lumen-cli`
  is pinned three tags below it (`tag = "v0.0.9-restart"`, its `Cargo.toml:12`).
- **Nothing since `2b1cd7f` is tagged, and `HEAD` is still not the commit to tag.** `2469b13`,
  `527a9c1`, `770537c` and `04ff377` are all committed and pushed but reachable only by branch, and
  the two newest fixes are uncommitted. **`770537c` is the newest *commit* that is safe to tag**;
  `04ff377` as committed carries the silent-success regression, so tagging it would ship a
  downloader that writes corrupt chunks and reports success. The working tree is safe — commit it
  first, then tag that.

`cargo build --lib` is free of rustc warnings. `cargo clippy --lib -- -W clippy::all` reports
**43 warnings**, unchanged in count and composition from the previous pass — neither `04ff377` nor
the working tree added or removed one, and the five same-type `backoff(attempt as u32)` casts are all
still present. See the [clippy](#low--style--clippy) section.

---

## High — downloader reliability

- [ ] **`ProgressRegression` has a doc comment, but not a contract — and the comment is wrong at
  most of its emit sites.** `04ff377` annotated both payload-carrying variants
  (`src/downloader/progress_reporting/download_event.rs:5,7`): "Used to indicate progress (bytes
  downloaded)" and "Used to indicate progress regression (a chunk has failed and has to be
  retried)". That is a real improvement over an undocumented variant, and it settles the units
  question — the payload is bytes, not a percentage or an absolute total. Two things it doesn't
  settle, both of which a consumer has to get right on the first try:
  - *"has to be retried" is false at three of the nine sites unconditionally, and at the other six
    whenever they fire on a terminal attempt.* `ChunkStreamCallbackError` (`:321`), `UrlParseError`
    (`:325`) and the `decoder.shutdown()` failure the working tree added (`:339`) never retry at
    all; the four transport arms and the two verification checks emit it immediately before
    `return Err` on their last attempt as well as before each `continue`. A consumer reading the
    comment will assume more `Progress` for that unit follows; often the unit is simply over.
  - *The arithmetic contract is still unwritten.* Nothing says the payload is a delta to
    **subtract** rather than a replacement total, and nothing warns that it can exceed what the
    consumer has accumulated for that unit. `lumen-cli` accumulates into a `u64`
    `state.downloaded_bytes` (`src/middleware/downloads.rs:618-620`), so a naive `-=` is a panic in
    debug and a wrap in release; the correct handling is `saturating_sub`. Say so on the variant —
    with `restart` carrying no `api.md` (see the coverage section), the doc comment is the only
    place this can live.

  `download`/`repair` still forward every `DownloadEvent` verbatim into
  `DownloadStageEvent::DownloadStage` (`downloader.rs:213-219` and `:132-138`) without interpreting
  any of them, and nothing in the crate ever matches the variant, so the consumer is the only place
  these semantics are exercised.

- [ ] **The per-failure-class budget is right now; the batch still can't short-circuit.** With
  `attempt < MAX_ATTEMPTS - 4` (`:348`) a size-correct digest mismatch against a bad manifest entry
  costs 3 transfers rather than 6, with ~0.75s of backoff on average (`backoff` ceilings 0.5s, 1s)
  instead of ~7.75s, while the `remaining() != 0` arm (`:356-373`) correctly keeps all six attempts —
  a short response is always transport-shaped. That is exactly the split the previous pass asked
  for, and this part of it is done.

  What is unchanged is that nothing short-circuits: `download_files` drains the whole
  `buffer_unordered` stream and only then collects the results (`:379-384`), so the first chunk's
  three failed attempts don't stop the remaining units from spending theirs. Against a stale
  manifest that is 3× the transfer for every affected chunk, discovered one chunk at a time. A first
  `ChunkHashMismatch` is evidence about the *manifest*, not about that one chunk, and could
  reasonably abort the batch.

- [ ] **`DownloadEvent::Progress` is emitted once per network read — the exact event-rate problem
  mainline `v0.1.1` was cut to fix, now on two code paths.** `download_files`'s callback sends
  `DownloadEvent::Progress(chunk_lenght)` for every `Bytes` the `bytes_stream()` yields
  (`src/downloader/downloader.rs:281-283`), into an unbounded channel that a forwarding task
  re-sends into another unbounded channel (`:213-219` for `download`, `:132-138` for `repair`). With
  `self.threads` chunks in flight on a fast connection that is thousands of sends per second,
  bounded by read syscall size rather than by throughput. `ProgressRegression` now has nine emit
  sites feeding the same channel, so the last three passes added to that traffic rather than reducing
  it.

  The workspace `CLAUDE.md` documents this same shape as the repair-download memory leak that
  `master`'s `v0.1.1` exists to fix: there, `gogdl_flutter`'s drain loops flooded an unbounded
  `StreamSink` faster than Dart could drain it, and the fix had two halves — coalescing in the bridge
  *and* cutting the rate at the source by moving `Progress` to the write-buffer flush boundary
  (`master:src/downloader/stream.rs:45-70` spells out the reasoning). The `restart` branch has
  neither half. Fix before any Flutter consumer is wired up to `repair_game`.

- [ ] **Secure links are never proactively expired.** `SecureLinksManager::links_cache`
  (`src/secure_links/links_manager.rs:19,32-63`) never evicts an entry on its own, and the fetched
  `CdnUrlParams`'s `expires_at`/`ttl` fields (`src/secure_links/secure_links.rs:11,13`) are parsed
  but never consulted. The reactive half works, and is cheap — a CDN 401 triggers
  `invalidate_secure_links` and an immediate retry with no backoff
  (`src/downloader/downloader.rs:306-318`) — but every chunk request still has to *fail* once before
  a stale link gets replaced, and when several chunks hit that 401 together the misses aren't
  collapsed (see the thundering-herd item below). Wants the same shape `Auth::is_valid()`'s
  60s margin already has for tokens.

## Medium — duplication & consistency

- [ ] **`ProgressRegression` is a second source-breaking addition for existing consumers, on the
  same pin bump as `VerificationStage`.** `lumen-cli` (sibling repo, pinned at
  `tag = "v0.0.9-restart"`, its `Cargo.toml:12`) matches `DownloadEvent` exhaustively with four arms
  and no `_` inside `apply_download_stage_event` (`src/middleware/downloads.rs:607-621`), so the new
  variant is a hard `E0004` there — as is `DownloadStageEvent::VerificationStage`
  (`src/downloader/progress_reporting/download_stage_event.rs:9`) in the outer match at `:555-607`.
  `DownloadError::ChunkHashMismatch()` (`src/downloader/error.rs:58-59`) widens a third `pub` enum,
  though error enums are less often matched exhaustively.

  Neither break is just a new arm: `lumen-cli`'s `DownloadStage` enum has no verification state to
  map `VerificationStage` onto, and its `downloaded_bytes` accumulator needs a `saturating_sub` to
  absorb `ProgressRegression`. Combined with the `E0599: no method named 'get_secure_links'` break
  the export item below flags, that pin bump needs a real code change on the consumer side. Same
  applies to `gogdl_flutter`'s `restart` branch whenever `repair_game` gets bridged.

- [ ] **`ChunkHashMismatch()` names the failure but carries none of it — and its sibling arm's
  message now has a dead branch.** `DownloadError::ChunkHashMismatch()`
  (`src/downloader/error.rs:58-59`) is a unit-shaped variant written with empty parens, formatting
  as the constant string "Chunk hash mismatch during download": no path, no offset, no expected or
  actual digest, so a consumer that hits it on one chunk of a multi-gigabyte install has nothing to
  report or retry against — and it is the *only* signal a consumer gets that a chunk was silently
  wrong rather than merely unreachable. The `remaining() != 0` arm three lines below does the
  opposite, building a fully detailed `io::Error` (`:363-372`) — two adjacent verification failures
  with opposite diagnostic quality.

  That detailed message is also still partly dead: it ends with
  `if actual_md5 == download_unit.md5 { "ok" } else { "mismatch" }` (`:370`), a leftover from when
  the two conditions were fused. The MD5 check at `:346` returns first, so `:370` is only reachable
  when the digests match and the string is always "checksum ok". Give `ChunkHashMismatch` the same
  fields (path, offset, expected, actual) and drop the dead conditional.

- [ ] **`MAX_ATTEMPTS` is now one constant, but the retry primitives are still split across two
  modules and reached by two import paths.** The duplicated declaration is gone:
  `src/constants/mod.rs:5` holds the single `pub const MAX_ATTEMPTS: u32 = 6;` (private at the crate
  root — `lib.rs:2` declares `mod constants;`, so nothing leaks), imported by
  `client.rs:15` and `downloader.rs:9`. Both loops read `for attempt in 0..MAX_ATTEMPTS`
  (`downloader.rs:264`, `client.rs:47`). **The uniform sentinel did not survive `04ff377`:** six of
  the seven are still `attempt != MAX_ATTEMPTS - 1` (`downloader.rs:291,300,311,330,358`,
  `client.rs:64`), while the MD5 arm is now `attempt < MAX_ATTEMPTS - 4` (`:348`). The working tree's
  `<` makes that arm safe on its own terms — every attempt at or past the bound returns, so it cannot
  fall through the loop the way `!= MAX_ATTEMPTS - 3` could — but it does confirm that one shared
  constant was never the invariant. "Every sentinel is the last-attempt test, spelled the same way"
  was, and it broke the first time anyone adjusted a budget. Three residuals:
  - *The constant and the function that consumes it live in different modules.* `MAX_ATTEMPTS` is in
    `constants`, alongside three GOG endpoint URLs it has nothing to do with; `backoff` is in
    `downloader::util`, re-exported by `downloader/mod.rs:23` beside the public event types and
    imported by the client layer as `crate::downloader::{FileType, backoff}` (`client.rs:16`) while
    `downloader.rs:9` imports the same function as `crate::downloader::util::backoff`. Two paths to
    one item, one of which makes the client depend on the downloader for a generic retry primitive.
    A `retry` module owning `MAX_ATTEMPTS` and `backoff` together — with a doc comment stating the
    base, ceiling and jitter policy, which nothing currently records — is now a smaller move than it
    was.
  - *A per-failure-class budget needs a per-failure-class constant.* The MD5 arm genuinely wants a
    smaller bound than the transport arms, and now has one — but it is spelled `MAX_ATTEMPTS - 4`,
    which reads as "four fewer than the transport budget" and means "three attempts". Nothing at the
    call site says 3, and the two numbers move together steeply: `MAX_ATTEMPTS = 6` gives the MD5
    arm 3 attempts, 5 gives it 2, 4 gives it 1 (no retry at all), and anything below 4 stops
    compiling (`arithmetic_overflow` is deny-by-default). A second named constant beside
    `MAX_ATTEMPTS` —
    `pub const MAX_HASH_ATTEMPTS: u32 = 3;`, with `if attempt + 1 < MAX_HASH_ATTEMPTS` — states the
    budget where it is read and decouples the two. The failure mode of the current spelling is
    "fewer retries than intended", never the silent success `04ff377` had, so this is a legibility
    fix, not a correctness one.
  - *`backoff`'s ceiling is still an unrelated literal, and still never binds.*
    `src/downloader/util/backoff.rs:4-6` caps at 20s, but `backoff` is only called when
    `attempt != MAX_ATTEMPTS - 1`, so the largest `attempt` it ever sees is 4 and the largest
    ceiling actually reachable is `500ms * 2^4` = 8s. The cap first binds at `attempt == 6`, so it
    is dead for any `MAX_ATTEMPTS <= 7`. Either derive the cap from `MAX_ATTEMPTS` or drop it and
    say so. The de-facto per-chunk timeout against a dead CDN — five backoffs, ~7.75s average and
    15.5s worst case, on top of six connection attempts — is still recorded nowhere.

- [ ] **The two retry loops are hand-copied rather than shared, and `download_files` carries a dead
  arm.** `download_files`'s match and `fetch`'s both independently carry an `HttpError{status, body}`
  arm and their own notion of what to retry; a change to one won't propagate to the other. And
  `download_files`'s `Err(ClientError::AuthError(err))` arm (`src/downloader/downloader.rs:288-296`)
  is unreachable: `stream_chunk` converts an auth failure during the secure-links fetch to
  `ClientError::SecureLinksError { inner: String }`, and `stream_chunk_inner` never calls
  `get_auth()` at all (CDN URLs are pre-signed, no bearer token), so a bare `AuthError` can never
  arrive here. It was copied from `fetch` and widened from `AuthError::TokenExpired` to any
  `AuthError`, so it now reads as if it handles more than it does — and each pass adds to it:
  `2469b13` gave it a `return Err(..)` and a `sleep`, `527a9c1` swapped that `sleep` for a `backoff`
  call and re-derived its sentinel from `MAX_ATTEMPTS`, and `770537c` added a `ProgressRegression`
  send to it (`:289`) — all of it on a path that cannot execute.

  The `HttpError` arm still tests `status == reqwest::StatusCode::UNAUTHORIZED` twice (`:308` to
  invalidate, `:312` to skip the backoff), with the retry `continue` written out in both branches
  (`:313`, `:316`). One test setting a `delay: bool`, or hoisting the check above the
  `if attempt != MAX_ATTEMPTS - 1`, says the same thing once. The two verification arms
  (`:346-354`, `:356-373`) then write the invalidate-and-back-off pair a third and fourth time —
  four match arms and two checks share one recovery shape, spelled out separately at each of six
  sites. The `ProgressRegression` send then had to be hand-pasted into all six plus three
  non-retrying exits, across three passes (`770537c` did four, `04ff377` four more, the working tree
  the ninth) — and the one site where `04ff377` also touched the *guard* while copying is where the
  silent-success regression got in, and where the working tree then had to fix it. Six
  hand-maintained copies of one recovery shape is how that keeps happening; a single
  `retry_or_return(attempt, bound)` helper would make each site one line and put each sentinel in
  exactly one place.

- [ ] **`Downloader::repair` is a near-verbatim copy of `Downloader::download`.** `repair`
  (`src/downloader/downloader.rs:42-141`) and `download` (`:142-222`) are the same function apart
  from one inserted stage. Lines `48-104` of `repair` and `148-204` of `download` — path-resolver
  construction, the `depot_files` flat_map, the whole file-size-verification stage with its
  channel/`tokio::join!`/forwarding boilerplate, the free-space computation and its two error
  returns, the whole allocation stage with the same boilerplate again, and the `FileAllocationError`
  abort — are identical modulo two extra `drop()`s and the stage-forwarding variable names. The only
  real difference is `repair`'s `verify_download_units` stage (`:106-125`), whose `missing_units` it
  passes to `download_files` where `download` passes `DownloadUnit::from_product_bundles(&bundles)`
  wholesale (`:208`).

  Every fix in this doc against `download`'s pipeline has to be applied twice, and the free-space
  error-discard item below already exists in two places because of it. `download` is expressible as
  `repair` with the verification stage skipped — or better, as a shared private helper taking a
  "which units to download" closure.

- [ ] **`download` re-downloads every chunk regardless of what's already correct on disk, so `repair`
  is the crate's only resume path — and nothing says so.** `download` runs a file-size verification
  stage and computes `missing_files`, then ignores that result when building the transfer list:
  `DownloadUnit::from_product_bundles(&bundles)` (`src/downloader/downloader.rs:208`) enumerates
  every chunk of every file in every bundle, including files that just verified as complete. An
  interrupted download restarted through `download_game` re-transfers the whole game from byte 0,
  while `repair_game` transfers only what fails MD5.

  Given pause/resume was deliberately removed chain-wide (see the workspace `CLAUDE.md`),
  `repair_game` is the de-facto resume entry point — and a consumer has no way to know that from the
  API surface: the two methods have identical signatures (`download_manager.rs:73-101`) and neither
  has a doc comment. Either make `download` skip already-verified units, or document that resuming
  means calling `repair_game`.

- [ ] **`repair` checksums the chunks of files it has just allocated.** Stage 2 allocates every file
  that failed size verification (`set_len` on a fresh or truncated file,
  `src/downloader/fs/path_resolver.rs:76`), and stage 3 then MD5s **every** unit of **every** file
  (`src/downloader/downloader.rs:106-118`) — including those just-allocated, all-zero ranges, whose
  chunks cannot possibly match. On a repair where a large file is missing outright that's a full-size
  read plus MD5 of zeroes purely to conclude what stage 1 already knew. Filter the units belonging to
  `missing_files` out of the verification pass and add them straight to the download list.

- [ ] **Secure-link fetches aren't collapsed across concurrent chunk downloads, and `a11273a`
  deleted the pre-fetch that used to hide it.** `SecureLinksManager::get_secure_links`/
  `invalidate_secure_links` (`src/secure_links/links_manager.rs:32-67`; cache at `:19`) have no
  in-flight-request dedup. `download_files` runs up to `self.threads` chunk downloads concurrently via
  `buffer_unordered` (`src/downloader/downloader.rs:379`), each calling `stream_chunk`
  independently, and `stream_chunk` calls `get_secure_links` per chunk (`src/client/client.rs:87`).
  - *Cold start (new with `a11273a`):* `download_files` used to warm the cache first — a
    `stream::iter(bundles).map(|b| get_secure_links(..)).buffer_unordered(self.threads)` block that
    `a11273a` removed along with the `(String, DownloadUnit)` tupling it sat beside. Nothing replaced
    it, so the first `self.threads` chunk tasks all miss the empty `links_cache` at once and each
    issues its own round-trip. The cache-check and the fetch are not under one lock
    (`links_manager.rs:39-62`), so the mutex doesn't collapse them either. **Per product bundle, on
    every download and every repair.**
  - *Expiry:* if a link expires with several chunks in flight, each hits the CDN 401 at roughly the
    same time, each calls `invalidate_secure_links`, and each re-fetches independently. The two
    verification retries (`:349`, `:359`) invalidate too, so a batch of chunks failing MD5 together
    produces the same storm.

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
  calls, so `inner_fetch` (`src/client/client.rs:128-149`) can **never** produce
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
  constructed `DeflateError` (`src/downloader/downloader.rs:273,340,363`). Confirmed
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
  - **One `#[tokio::test]` driving `download_files` against a scripted chunk source.** Three cases
    off one harness, in descending order of what they'd have saved:
    - *Fails every attempt* → asserts `Err`. Catches the swallowed-error regression on the day it
      was written, both times, and fails today against `v0.0.12-restart`.
    - *Fails transport on attempts 0–4, then serves wrong-MD5 bytes on attempt 5* → asserts `Err`.
      Gets `Ok` against `04ff377` as committed; passes against the working tree. **This is the one
      to write first** — it is one line different from the case above, it is the only thing that
      pins the new `attempt < MAX_ATTEMPTS - 4` sentinel, and the class it covers has now recurred
      four times.
    - *Wrong-MD5 bytes on the first attempt, good bytes on the second* → pins the MD5 retry, and
      asserting the net reported byte total equals exactly one chunk pins `ProgressRegression`'s
      arithmetic, which no consumer-side test can pin for the producer.
  - **`backoff`'s bounds** (`src/downloader/util/backoff.rs`) — the ceiling is deterministic even
    though the draw isn't, so `attempt` → ceiling is a plain table test, and a single assertion that
    the ceiling at `MAX_ATTEMPTS - 2` is the cap would have caught the still-dead 20s `.min(..)`
    when the helper was written.
  - Auth logic, all unit-testable without network: `Auth::is_valid` boundaries (including the 60s
    buffer), the `to_string`/`from_string` restore round-trip (regressed once in `998829d`, fixed in
    `6c76f03`, still unpinned), `refresh_auth` persisting `valid_until` (broke twice across
    refactors), and concurrent `refresh_auth` collapsing.

  For contrast, `lumen-cli` already has a `#[cfg(test)] mod tests` exercising
  `apply_download_stage_event` against synthetic `DownloadStageEvent` sequences
  (`src/middleware/downloads.rs:1117-1211`) — the consumer of these events is better covered than
  the producer.

- [ ] **No `CLAUDE.md`/`api.md` on the `restart` branch.** Both exist on `master` and are treated as
  canonical specs for consumers (`gogdl_flutter`); `restart` has neither, so there's no single
  reference tracking what public surface has been rebuilt vs. still stubbed. Each pass sharpens
  this: `a11273a` added a public `GogDl::repair_game` and a `DownloadStageEvent` variant with no doc
  comment, and `770537c`/`04ff377` added a `DownloadEvent` variant whose one-line doc comment states
  neither its arithmetic nor when it can arrive, and describes a retry that three of its nine emit
  sites never perform (see the semantics item above). The download-vs-repair semantics (which resumes,
  which re-transfers, which stages each emits) are discoverable only by reading `downloader.rs`.

## Low — style / clippy

- [ ] **`open_file`'s `?` is the last exit from the attempt body that doesn't cancel its reported
  bytes.** Eight of the nine exits now emit `ProgressRegression(reported_bytes)`
  (`downloader.rs:289,298,307,321,325,329,339,347,357`); `path_resolver.open_file` (`:268`)
  propagates with `?` and no event. It runs before a single byte is reported, so the amount leaked
  is always 0 and there is no observable effect — worth closing anyway so the invariant reads "every
  exit from the attempt body cancels the bytes it reported, no exceptions" rather than "every exit
  but one, and you have to know why".

- [ ] **Vestigial `let _ = body;` — one of the two is left.** `770537c` deleted `downloader.rs`'s
  copy; `HttpClient::fetch` still has the identical pair, `client.rs:51` discarding a `body` that
  `:57` returns as `body: body`. Delete both together.

- [ ] **`chunk_lenght` is misspelled.** `src/downloader/downloader.rs:281-283`, introduced by
  `770537c` when the callback needed to name the length twice. Rename before it gets copied.

- [ ] **The free-space check discards its own error detail, in two places.** `Downloader::download`
  (`src/downloader/downloader.rs:175-178`) and its copy in `Downloader::repair` (`:76-79`) each match
  `path_resolver.get_free_space()`'s `Err(_)` and always return the unit variant
  `DownloadError::CouldNotResolveFreeSpace`, throwing away the
  `FileSystemError::NoDiskMatchingPath(PathBuf)` that names exactly which resolved base path had no
  matching disk (`src/downloader/fs/path_resolver.rs:39-58`). Carry the path (or the whole
  `FileSystemError`) into the variant. Same shape as `ChunkHashMismatch()` above — a unit error
  variant where the caller needed a name.

- [ ] **`rand` with default features is a heavy dependency for retry jitter.** `rand = "0.10.2"`
  (`Cargo.toml:22`) pulls `chacha20`, `getrandom 0.4`, `rand_core 0.10` and `r-efi` — and with
  `chacha20` comes `cpufeatures`, which `2100296` had removed from the tree along with `sha2`. A
  ChaCha20 CSPRNG for a sleep duration is more than the job needs: a non-cryptographic generator, or
  jitter derived from the chunk hash the task already holds, keeps the dependency graph where
  `2100296` left it.

**Clippy: 43 warnings** against this tree — same count and same composition as the previous pass;
neither `04ff377` nor the working tree added or removed one. Locations re-derived:

- [ ] **5 `unnecessary_cast`.** `attempt` is `u32` (it comes from `0..MAX_ATTEMPTS`), so
  `backoff(attempt as u32)` casts `u32` to `u32` at `client.rs:65` and
  `downloader.rs:292,301,315,331`. The two verification arms (`downloader.rs:350,360`) write the
  bare `backoff(attempt)`, so the five casts are also inconsistent with the other two call sites.
- [ ] 7 `let_and_return` — `download_manager.rs:70,85,100`, `downloader.rs:443,531,616`,
  `product_bundle.rs:47`.
- [ ] 4 "redundant redefinition of a binding `path_resolver`" — `downloader.rs:259,395,454,542`
  (each warning also points at the shadowed parameter, at `:251,390,449,537`), all the same
  `let path_resolver = path_resolver;` idiom moving it into a closure.
- [ ] 5 module inception — `client::auth::auth`, `client::client`, `downloader::downloader`,
  `gogdl::gogdl`, `secure_links::secure_links`.
- [ ] 4 `needless_return` — `client.rs:143,146`, `downloader.rs:604`, `path_resolver.rs:49`.
- [ ] 3 `redundant_field_names` — `client.rs:57` (`body: body`), `depot_info.rs:58`
  (`offset: offset`), `downloader.rs:304` (`inner: inner`).
- [ ] 3 `len_zero` — `downloader.rs:100,121,199`.
- [ ] 2 `needless_borrow` on `refresh_auth(&self)` — `client.rs:54,61`.
- [ ] 2 `manual_map` — `auth_manager.rs:113` (clone-observer-out-of-guard, wants `.cloned()`),
  `depot_info.rs:39` (`DepotFile::size`, wants `.as_ref().map(..)`).
- [ ] 2 `or_insert_with(Vec::new)` → `or_default()` — `downloadable_product.rs:50`,
  `product_bundle.rs:42`.
- [ ] `Auth::is_valid`'s `map_or(false, ..)` should be `is_some_and(..)`
  (`client/auth/auth.rs:33`).
- [ ] `OwnedGames::default()` is a hand-written inherent method shadowing `std::default::Default`
  (`games/owned_games.rs:14`) — implement the trait.
- [ ] Useless `format!` with no interpolation on `format!("https://embed.gog.com/user/data/games")`
  (`games/owned_games.rs:25`).
- [ ] One-offs: explicit-closure-for-cloning (`depot/build_metadata.rs:38`), `io_other_error`
  (`downloader/util/hash.rs:44`), redundant `&` in a `format!` (`depot/depot_info.rs:80`).

`cargo clippy --fix --lib -p gogdl-lib -- -W clippy::all` auto-applies 33 of these — but only
against a clean tree; it will otherwise try to "fix" whatever is mid-edit.

**Clippy caught none of the six retry defects this branch has shipped**, including the newest.
`for attempt in 0..2` against `attempt != 2` and `for attempt in 0..3` against `attempt != 2`
produce byte-identical clippy output; so do `attempt != MAX_ATTEMPTS - 1`,
`attempt != MAX_ATTEMPTS - 3` and `attempt < MAX_ATTEMPTS - 4`, even though the middle one lets the
loop run off its end into `Ok(())` and the other two don't. A `for` loop missing its `break` fires no
lint; the `return`s an always-true
guard renders dead are unreachable only at runtime; two consecutive `sleep`s — the doubled delay
`fetch` briefly carried — are perfectly legal code; and a `tx.send(..)` present in four match arms
and absent from two sibling `if` blocks is invisible to any lint. What clippy *did* notice about
this retry code is five redundant casts. Only reading the code, or a test, tells the real defects
apart.

---

## Closed

One line per fixed item, newest first within each group. Detail is in the referenced commits.

### Downloader reliability

*The first two are in the **uncommitted working tree**, not in any commit or tag. The three after
them are in `04ff377` and `770537c`, then `527a9c1` and the five after it — all committed and pushed,
all above `v0.0.12-restart`, none tagged.*

- [x] **A chunk whose MD5 mismatch first appeared on a late attempt was reported as a successful
  download** — working tree. `04ff377` had changed the MD5 sentinel to `attempt != MAX_ATTEMPTS - 3`,
  an equality against a fixed interior index (3), so a mismatch first seen on attempt 4 or 5 — after
  transport failures had spent the earlier attempts — took the `continue`, ran off the end of the
  `for` loop and fell through to `Ok(())`, leaving the bad bytes on disk and the install reported
  complete. Now `attempt < MAX_ATTEMPTS - 4` (`downloader.rs:348`): a comparison, so attempts 2–5
  all `return Err(ChunkHashMismatch())` and the loop has no fall-through. The fourth appearance of
  this bug on the branch (`f3a944a` → `ac061e0` → that commit's `0..2` bound → `2469b13` → here),
  and still nothing tests it — see the coverage section. That the bound is still written as an
  offset from `MAX_ATTEMPTS` is open above, as legibility only.
- [x] **A zlib error in `decoder.shutdown()` leaked a full chunk's worth of reported bytes** —
  working tree. The bare `?` became `if let Err(err) = decoder.shutdown().await` emitting
  `ProgressRegression(reported_bytes)` before returning (`downloader.rs:338-341`). `open_file`'s `?`
  is the ninth exit and the only one left; it always leaks 0, and is open under Low.
- [x] **The two verification retries, and the two non-retryable client-error arms, never emitted
  `ProgressRegression`, so a chunk that streamed to completion and then failed verification
  over-reported its full size on every attempt** — `04ff377`, completed by the working tree above.
  **That same commit's MD5 sentinel change is the regression in the first Closed entry** — this line
  is not a reason to tag `04ff377` as it stands.
- [x] **`MAX_ATTEMPTS` was declared twice, once per file, agreeing only by coincidence** —
  `770537c`. One `pub const MAX_ATTEMPTS: u32 = 6;` in `src/constants/mod.rs:5`, imported by
  `client.rs:15` and `downloader.rs:9`; `mod constants` is private in `lib.rs`, so nothing leaks from
  the crate root. That it now sits apart from `backoff`, that `backoff`'s ceiling still isn't derived
  from it, and that a *second* bound is now derived from it by subtraction, are open above.
- [x] **Bytes streamed by an attempt that later failed were counted as progress, with nothing to
  undo them** — `770537c` for the four transport arms
  (`DownloadEvent::ProgressRegression(reported_bytes)` at `downloader.rs:289,298,307,329`, where
  `reported_bytes` (`:278`) resets per attempt), extended to nine sites by `04ff377` and the working
  tree above.
- [x] **The MD5 and length checks were fused, so a size-correct digest mismatch and a truncated
  response were indistinguishable to the caller** — `770537c`. Two independent `if`s at
  `downloader.rs:346-354` and `:356-373`, the former returning the new
  `DownloadError::ChunkHashMismatch()`. Its missing detail and the now-dead conditional in the
  latter's message are open above.
- [x] **Vestigial `let _ = body;` in `download_files`'s `HttpError` arm** — `770537c`.
  `HttpClient::fetch`'s copy (`client.rs:51`) survives; open above.
- [x] **A chunk failing its MD5/length check was never retried, failing the whole download on the
  first bad byte** — `527a9c1`. Invalidates the secure links, backs off and `continue`s.
- [x] **The MD5/length retry hit the same edge node immediately, with no delay and no link
  invalidation** — `527a9c1`, in the same pass that introduced it.
- [x] **The retry delay was linear and unjittered, so every in-flight chunk task retried in
  lockstep** — `527a9c1`. `src/downloader/util/backoff.rs` draws a full-jitter delay from
  `0..=min(500ms * 2^attempt, 20s)`; every retrying site uses it. The dead 20s `.min(..)` is open
  above.
- [x] **`HttpClient::fetch` slept twice per network retry** — `527a9c1`, in the pass that introduced
  it: `backoff(..)` had been *appended* to the linear `tokio::time::sleep` rather than replacing it,
  so a retried fetch waited 2s + jitter, then 4s + jitter. The `sleep` and the now-unused
  `time::Duration` import are both gone.
- [x] **The retry bound and its five last-attempt sentinels were independent literals, in two
  loops** — `527a9c1`. `attempt != MAX_ATTEMPTS - 1` at every sentinel. Re-opened by `04ff377`,
  which replaced one of those sentinels with `attempt != MAX_ATTEMPTS - 3`; closed again by the
  working tree's `attempt < MAX_ATTEMPTS - 4`. The constant survived both; the uniform *shape* that
  made the regression class impossible did not, and a comparison rather than a matching spelling is
  what carries the guarantee now.
- [x] **A routine CDN 401 paid the same congestion backoff as a downed CDN** — `527a9c1`. The
  `HttpError` arm invalidates the secure links and retries immediately when
  `status == UNAUTHORIZED` (`:306-318`). The unreachable `AuthError` arm and the reachable
  `SecureLinksError` arm still back off; the former is dead code (tracked above), the latter covers
  a failed secure-links *fetch*, which is transport-shaped, so backing off there is defensible.
- [x] **Both retry loops off by one (`0..2` bound tested against `!= 2`), making every last-attempt
  branch dead code** — `2469b13`. Both bounds back to `0..3`; all five `return Err(..)` reachable.
  Made structurally impossible afterwards by `MAX_ATTEMPTS` above.
- [x] **A chunk that fails every attempt was reported as a successful download** — introduced by
  `f3a944a`, fixed in `ac061e0`, re-introduced by the same commit's `0..2` bound, closed by
  `2469b13`. Failures now propagate through `buffer_unordered` with full detail.
- [x] **The retry loop never broke, so every chunk was downloaded three times** — `ac061e0`. `break`
  at `:374`, after `decoder.shutdown()`/MD5 verification, so the successful attempt is still
  verified. (`04ff377` reopened the *other* way out of that loop — first Closed entry.)
- [x] **Local, non-retryable failures burned all three attempts** — `7eb5d5e`.
  `ChunkStreamCallbackError` (full disk, over-length guard, zlib error) and `UrlParseError` return on
  first occurrence, above the catch-all, with no `attempt` guard.
- [x] **No retry/backoff for transient network failures** — `2b1cd7f` (arms + delay), `2469b13`
  (delay on all four `download_files` arms, linear `(attempt+1)*2`, none on the way out), then
  `527a9c1`'s jittered `backoff` helper replacing that delay and raising 3 attempts to 6.
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
