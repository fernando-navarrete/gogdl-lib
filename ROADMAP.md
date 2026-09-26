# ROADMAP

The order in which the items in [GAPS.md](GAPS.md) will be fixed, and the version tag each one ships
under. GAPS sections are referenced by title (e.g. GAPS "Medium — cloud saves"), since they aren't
numbered.

## Versioning rules

- Everything stays on **1.x.x**.
- **Patch** (`1.x.y`): one tag per small, self-contained fix or addition.
- **Minor** (`1.x.0`): a big update, meaning a feature set or cross-cutting refactor. Patches then
  continue from `.1` under that minor.
- Tags use the existing `vX.Y.Z` format. `v1.1.0`–`v1.1.6` and `v1.2.0`–`v1.2.3` shipped, so the
  next stage is **`v1.3.0`**. The old `-restart`/`-debug` tags stay, but nothing new is tagged that way.
- For each release: bump `version` in `Cargo.toml` to match the tag (the `v1.0.1` mismatch in GAPS
  "Low — style / clippy" must not happen again), tick the item(s) in `GAPS.md`, add a
  `CHANGELOG.md` entry, commit, then tag.
- Consumers pin this crate by **git tag**, not by semver range, so a breaking API change is allowed
  in a **minor**, never in a patch. It ships together with the bridge change that adopts it, and
  the entry below says so. A change of *behavior* behind an unchanged signature (like `v1.0.11`'s
  `get_owned_games`) counts as breaking for this rule and is called out in the `CHANGELOG.md` entry.
- Additive public API (a new method, a new re-export) is fine in a patch.

## Lining up with consumers

Three repos depend on `gogdl-lib`:

- **`gogdl_flutter`** (the Flutter/Rust bridge) pins `v1.0.11`. Its GAPS §7 is the list of requests
  it makes here, and its `ROADMAP.md` orders them by the Lumen stage that needs them.
- **Lumen** uses this crate only through the bridge. Its `v1.3.0-DOWNLOAD-MANAGEMENT.md` lists
  requests D4–D6 here.
- **`lumen-cli`** pins `v1.0.10` and exercises this crate directly. It has no roadmap of its own: it
  follows the newest tag and shows what works. Bumping it is the smoke test for each release.

So every release after the foundation is ordered by the bridge release that needs it
(`../gogdl_flutter/ROADMAP.md`), which in turn follows Lumen's (`../lumen/ROADMAP.md`). A minor here
is tagged **before** the bridge starts the stage it serves, so the bridge can pin it on day one.
Where the bridge has a fallback for a missing item, the bridge doesn't wait. It picks up the fix
later as its own patch.

| Lumen stage | Bridge release | Needs from `gogdl-lib` (bridge GAPS §7 / Lumen D#) | `gogdl-lib` release |
|---|---|---|---|
| (none, internal) | `v1.2.0` ✅ | Test-constructible types (§7.1) | `v1.1.0` ✅ |
| `v1.3.0` Download management | `v1.3.0` | Cancel-safe contract (§7.2), download error kinds (§7.3), free-space query and mount fix (§7.4, D4, D5), no save truncation (§7.6 part), per-product sizes (§7.10), Proton cleanup on failure (D6) | `v1.2.0` ✅ |
| `v1.4.0` Library and account | `v1.4.0` | `logout` (§7.5), owned games that don't silently shrink, cheaper title and DLC lookups | `v1.3.0` |
| `v1.5.0` UI polish and packaging | `v1.5.0` | Login error kinds (§7.3 auth part), breaking error and API cleanup | `v1.4.0` |
| `v1.6.0` Extras | — | Nothing | — |
| Not on Lumen's roadmap yet (cloud saves) | `v1.6.0` | Save sync metadata, `cloudStorage.enabled`, file subsets (§7.6) | `v1.5.0` |
| Not on Lumen's roadmap yet | `v1.7.0` | Language/OS selection (§7.7), Proton integrity (§7.8), `i64` ids (§7.9) | `v1.6.0` |

Things consumers need that don't need a change here: resume is already `repair_game` (Lumen
`v1.3.0` "B"), and dropping a game job's future already stops it (no `tokio::spawn`), which
`v1.2.0` only has to document. Update detection uses `get_game_builds` as it is today.

**Settled:** Lumen's plan lists D4/D5 (free space) as blocking, while the bridge's `v1.3.0` plan ships
a `statvfs` fallback and doesn't wait. They were cut early as the additive patch **`v1.1.6`**
(`get_free_space`, the longest-mount fix and the "retires" typo), so Lumen can pin it without
waiting for `v1.2.0`.

---

## v1.1.0 — Foundation: tests, CI and CD ✅

This goes before any feature work so that everything after it lands with tests and a pipeline.
GAPS' first "standing fact" is that nothing on the download, auth or client paths is tested, and
the bridge's CI already can't catch a `gogdl-lib` regression until it pins a new tag.

- Refresh GAPS' header: it still describes `e032b7d` / `1.0.10`, but `v1.0.11` is tagged at `HEAD`.
- Fix the six rustc warnings and the 42 clippy warnings (38 auto-fixable), and run `cargo fmt`, so
  the new gates start green (GAPS "Low — style / clippy"). Pure style only. The misspelled
  `produt_type` and the `manual_filter_map` in `owned_games.rs` get the mechanical fix now; the
  silent-drop behavior stays for `v1.3.0`.
- A local HTTP server test fixture (e.g. `wiremock` or a small `hyper` server) under `tests/` or a
  `#[cfg(test)]` module, so network-facing code can be tested offline.
- Tests for `download_files`, in the order GAPS "Medium — missing coverage" gives: transport
  failures on attempts 0–4 then wrong MD5 on attempt 5 (`Err`), every attempt failing (`Err`), wrong
  MD5 then good bytes (retry plus `ProgressGuard`'s net byte total), one unit failing terminally
  mid-batch (`6b4b7f3`'s short-circuit, cancelled units net to zero), and `ProductNotOwned` from
  the secure-link fetch (characterizes today's six retries; `v1.2.0` changes it).
- Table test for `backoff`'s bounds. Auth tests with no network: `Auth::is_valid` and
  `SavesAuth::is_valid` boundaries (written against today's wrong-sign margin, flipped in
  `v1.1.2`), the `to_string`/`from_string` round-trip, `refresh_auth` persisting `valid_until`.
- Deserialization tests against captured responses: `SaveFile`, `RemoteConfig`,
  `ProtonGeRelease`/`ProtonGeReleasesPage` (including `get_suitable_asset` with `aarch64` and
  `.sha512sum` siblings), and the `gamesdb` response for a game, a DLC and a pack.
- Test-constructible public types for the bridge (its GAPS §7.1): make sure `GameBuild`,
  `ProductDetails`, `ProtonGeRelease` and the event enums can be built from JSON fixtures, and
  document that in their rustdoc. The bridge then adds its skipped model adapter tests.
- Pin the toolchain in `rust-toolchain.toml`, matching the bridge's (`1.98.1`), so this crate and
  the bridge build with the same compiler.
- CI: GitLab CI on the self-hosted runner (`thinkcentre.home`), reusing the bridge's setup: a
  prebuilt image (`ci/Dockerfile` + `ci/build-image.sh`, Rust only, no Flutter), `extra_hosts` for
  the LAN, and `pull_policy: if-not-present`. No deploy key is needed, since this crate has no
  private dependencies. Jobs:
  - `lint`: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`.
  - `test`: `cargo test`.
  - `doc`: `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps` (keeps `#![warn(missing_docs)]` and
    intra-doc links honest).
  Same trigger policy as the bridge and Lumen (tags only), plus manual runs on branches.
- CD, on a `vX.Y.Z` tag: fail if `Cargo.toml`'s `version` and the tag disagree, then create a
  GitLab release with that version's `CHANGELOG.md` section.
- Add `CHANGELOG.md`, backfilled with one line per `v1.0.x` tag (saying that `v1.0.11` changed what
  `get_owned_games` returns).

Follow-up patches (`v1.1.1+`), one per tag:
1. `v1.1.1`: stop refresh tokens leaking into error strings: `.without_url()` on the token-bearing
   requests in `refresh_auth` and `SavesAuth::get_saves_auth` (GAPS "Medium — cloud saves").
   Security, so it goes first.
2. `v1.1.2`: fix the wrong-sign 60s margin in `Auth::is_valid` and `SavesAuth::is_valid` as one
   helper, and flip the `v1.1.0` tests (GAPS "Low — style / clippy").
3. `v1.1.3`: an unauthenticated 401 returns `HttpError` at once instead of six back-to-back retries
   (GAPS "Medium — Proton").
4. `v1.1.4`: a manual `downstream` CI job that builds `lumen-cli` and the bridge's `rust/` crate
   against this commit (a `[patch]` override), so a breaking change shows up before the tag. Needs
   read-only deploy keys for both repos.
5. `v1.1.5`: fix the stale docs (User-Agent requirement, crate overview, saves internals) and add a
   `CLAUDE.md` that lists which capabilities exist (GAPS "Medium — Proton", "Medium — missing
   coverage").

## v1.2.0 — Download management (for bridge `v1.3.0` / Lumen `v1.3.0`) ✅

Everything the bridge's `v1.3.0` plan asks for in its "Dependencies on `gogdl-lib`" table, plus
Lumen's D4–D6. The bridge ships fallbacks for all of them, and picks each one up as a `v1.3.x`
patch, so this doesn't block it. It should still land before Lumen finishes its `v1.3.0`.

- `GogDl::get_free_space(path) -> Result<u64, GogDlError>`, the same number the download's own
  pre-flight check uses (bridge §7.4, Lumen D4). *Shipped early in `v1.1.6`.*
- `PathResolver::get_free_space` picks the **longest** matching mount point, not the first. Lumen
  confirmed that `/` wins over `/media/gamedisk` today (Lumen D5). *Shipped early in `v1.1.6`.*
- `NotEnoughFreeSpace` carries required and available bytes, and `CouldNotResolveFreeSpace` keeps
  the path it failed on (GAPS "Low — style / clippy", bridge §7.3).
- Export `FileSystemError`, `DepotFile` and `Chunk`, so consumers can match on which filesystem
  failure happened (GAPS "Medium — duplication & consistency", bridge §7.3). `DownloadUnit` stays
  internal: no public signature exposes it.
- Mark the event enums, `FileSystemError` and the download, Proton and saves error enums
  `#[non_exhaustive]`, so later patches (`v1.2.3`'s `DownloadEvent::Started`) can add variants
  without breaking a `match`. **Breaking** once, here.
- Fix the "Max retires reached" typo in `ClientError`'s `Display` (Lumen D7). *Shipped early in `v1.1.6`.*
- Document the cancel-safe contract in the rustdoc of `download_game`, `repair_game`,
  `verify_files`, `download_proton_release` and the saves transfers: dropping the future stops the
  work and leaves state `repair_game` can resume. Add a test that drops a `download_files` future
  mid-batch and then repairs (bridge §7.2).
- `download_save_files` writes to a temp file, `fsync`s and renames, instead of truncating a good
  save first. Data loss, and cancel makes it easier to hit (GAPS "Medium — cloud saves", bridge
  §7.6 part).
- `download_proton_release` extracts into a staging directory and removes it on failure or drop,
  then removes and renames. This also fixes the overlay-on-redownload bug (GAPS "Medium — Proton",
  Lumen D6).
- A cheap per-product size query from build metadata (per-depot `size`/`compressed_size`), without
  fetching every manifest (bridge §7.10): `GogDl::get_product_sizes` returning `ProductSize`s.
- Deterministic secure-link failures (`IncorrectGameId`, `ProductNotOwned`) return at once instead
  of six backoffs. Pull `MAX_ATTEMPTS`, `backoff` and a `retry_or_return` helper into one `retry`
  module, and delete the dead `AuthError` arm (GAPS "Medium — duplication & consistency").
- **Breaking** for bridge `v1.3.0`: the new fields on the free-space errors and the Proton staging
  behavior. Nothing is removed.

Follow-up patches (`v1.2.1+`), one per tag:
1. `v1.2.1`: `repair` skips checksumming the files it just allocated (resume speed for Lumen's
   "resume interrupted downloads").
2. `v1.2.2`: `download` and `repair` share one pipeline instead of two copies (GAPS "Medium —
   duplication & consistency"). "`download` skips chunks that already verify" moved to `v1.3.0`.
3. `v1.2.3`: `DownloadEvent::Started { compressed_total }` gives `Progress` a denominator so a
   percentage can reach 100%, and `Preparing`/`Prepared` are deprecated in its favor (still sent;
   removed in `v1.4.0`).

## v1.3.0 — Library and account (for bridge `v1.4.0` / Lumen `v1.4.0`)

- `download_game` skips chunks that already verify, so it resumes like `repair_game` (GAPS "Medium
  — duplication & consistency"). A **behavior change**: its events gain a `VerificationStage`. Moved
  here from `v1.2.2`, since a patch can't change the event stream.
- `GogDl::logout` (or `clear_auth`), which also drops the token observer (bridge §7.5). Without it
  the bridge falls back to `GogdlApi::reset()`.
- `get_owned_games` stops silently dropping failed lookups: return the unresolved IDs next to the
  games (an `unresolved` field on `OwnedGames`). `lumen-cli` asked for this directly (GAPS "Medium —
  owned games").
- Cache the per-product `gamesdb` type lookups on `GamesManagerInner` (failures not cached), and
  reconcile the `gamesdb` "game" check with `GamesError::ProductNotAGame`. This makes the bridge's
  batched title lookup and Lumen's library refresh cheap (GAPS "Medium — owned games").
- "Not a game" handling is the same across `GameBuilds`, `GameLinks`, `GameSummary` and
  `OwnedGames`, so `get_game_builds` on a DLC returns `ProductNotAGame`, not a decode error. Needed
  for Lumen's DLC tab (GAPS "Medium — duplication & consistency").
- Collapse concurrent secure-link fetches (in-flight dedup per `game_id`), and stop
  `SecureLinksManager` and `SavesManager` holding their mutex across network calls (GAPS "Medium —
  duplication & consistency", "Medium — cloud saves").
- Cache `DepotInfo`, `BuildMetadata` and `RemoteConfig`.
- **Breaking** for bridge `v1.4.0`: the new `OwnedGames` field (and `lumen-cli`, which gets its
  first bump since `v1.0.10`).

Follow-up patches (`v1.3.1+`), one per tag:
1. `v1.3.1`: `GameScreenshots::resolve_links` stops using the magic `formatter.get(2)`.
2. `v1.3.2`: populate or drop `GameDetails.id` and `GameBuilds.game_title`.

## v1.4.0 — Errors and API cleanup (for bridge `v1.5.0` / Lumen `v1.5.0`)

The bridge's `v1.5.0` is its breaking cleanup, so the breaking error changes here ship with it and
the bridge migrates once.

- A status-aware `AuthError`: invalid or expired login code, revoked refresh token, and network
  failure are separate variants. This is what Lumen's login screen needs for "accurate errors"
  (bridge §7.3 auth part, GAPS "Medium — duplication & consistency").
- `GogDlError::SavesError(#[from] SavesError)`, and the four saves methods return `GogDlError`
  (GAPS "Medium — cloud saves").
- Remove `DownloadEvent::Preparing` and `Prepared`, deprecated in `v1.2.3`.
- Remove the dead `Http`/`UrlParseError`/`NetworkError`/`DecodeError`/`DeflateError` variants, and
  `FileSystemError`'s `FileMetadataError` and `FileCreationError`, which no public method returns.
- Remove `SaveFile::relative_path`, and either export or un-`pub` the path sanitizers from
  `a813975` (GAPS "Low — style / clippy").
- **Breaking** for bridge `v1.5.0`: the error enums and the removed items.

Follow-up patches (`v1.4.1+`), one per tag:
1. `v1.4.1`: one `stream_url`/`send_checked` pair in `HttpClient` instead of the copied bodies, and
   a shared const for the GitHub headers.
2. `v1.4.2`: an `AsyncWrite` sink for `stream_chunk`, removing the per-read box and lock, and the
   double clones in `engine.rs`.
3. `v1.4.3`: trim `rand` and `tar` default features.

## v1.5.0 — Cloud saves (for bridge `v1.6.0`)

Not on Lumen's roadmap yet. The bridge's `v1.6.0` asks Lumen to add it first, and this release
should be tagged before that starts.

- Download and upload check `is_supported()` / `cloudStorage.enabled` (GAPS "Medium — cloud saves",
  bridge §7.6).
- Both transfers take a file subset, or return a plan the caller confirms, and expose local and
  remote timestamps/hashes per file so the bridge can detect conflicts (bridge §7.6).
- Capture a real `__default` listing and pin upload naming to it, and reject two listed names that
  resolve to one local path (GAPS "Medium — cloud saves").
- The saves API refreshes an expired access token like every other method, and a 401 evicts the
  cached `SavesAuth`.
- Verify against `SaveFile.hash` when there's no `ETag`, and move (de)compression off the async
  runtime (GAPS "Low — style / clippy").
- **Breaking** for bridge `v1.6.0`: the transfer signatures.

## v1.6.0 — Upstream-driven (for bridge `v1.7.0`)

Items with no Lumen stage yet. Each can ship as a patch as soon as it's ready.

- Language and OS selection for builds, metadata and `RemoteConfig` (bridge §7.7, GAPS "Medium —
  duplication & consistency").
- Check Proton tarballs against the `.sha512sum` asset while streaming (bridge §7.8, GAPS "Medium —
  Proton").
- `i64` (or `u64`) product ids (**breaking**, lockstep with the bridge and Lumen) (bridge §7.9).
