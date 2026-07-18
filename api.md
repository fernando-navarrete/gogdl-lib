# gogdl-lib2 API Reference

`gogdl-lib2` is a Rust library for interacting with GOG's (Good Old Games)
Galaxy backend: authenticating, browsing a user's catalog, downloading and
repairing game installations, managing cloud saves, and downloading Proton-GE
releases. This document is the complete specification of its public surface —
every type, function, and flow a consuming project needs.

- Crate edition: 2024
- Async runtime: [tokio](https://docs.rs/tokio) (the crate depends on tokio
  `"full"`; your binary needs its own tokio runtime — e.g. `#[tokio::main]`)
- HTTP: you supply the `reqwest::Client` (re-exported as `gogdl_lib2::Client`)

```toml
[dependencies]
gogdl-lib2 = { path = "..." } # or a registry/git dependency, per your setup
tokio = { version = "1", features = ["full"] }
reqwest = { version = "0.13", default-features = false, features = ["json", "rustls", "stream"] }
```

## Table of contents

1. [Core concepts](#1-core-concepts)
2. [Getting started](#2-getting-started)
3. [Authentication](#3-authentication)
4. [Game catalog](#4-game-catalog)
5. [Download flow](#5-download-flow)
   - [5.1 Discovering what to download](#51-discovering-what-to-download)
   - [5.2 Downloading (`download_files`)](#52-downloading-download_files)
   - [5.3 Pause / resume / cancel (`DownloadControl`)](#53-pause--resume--cancel-downloadcontrol)
   - [5.4 Repairing an install (`repair_files`)](#54-repairing-an-install-repair_files)
   - [5.5 Verifying an install (`verify_files`)](#55-verifying-an-install-verify_files)
   - [5.6 Tuning network behavior (`DownloadConfig`)](#56-tuning-network-behavior-downloadconfig)
6. [Cloud saves](#6-cloud-saves)
7. [Proton-GE releases](#7-proton-ge-releases)
8. [Error handling](#8-error-handling)
9. [Type reachability — what you can and can't name](#9-type-reachability--what-you-can-and-cant-name)
10. [Full `GogDl` method index](#10-full-gogdl-method-index)

---

## 1. Core concepts

**`GogDl` is the only entry point.** Every operation the library offers is a
method on `GogDl` — you never construct or touch the internal managers
(`AuthManager`, `DepotManager`, `GamesManager`, `DownloadManager`,
`SecureLinksManager`, `SavesManager`, `ProtonManager`, `HttpClient`) directly;
they are private implementation detail. `GogDl` is not `Clone`, so hold it
behind an `Arc` if you need to share it across tasks (all its methods take
`&self`, so an `Arc<GogDl>` works without extra synchronization on your end).

**One error type wraps everything.** Every fallible `GogDl` method returns
`Result<T, GogDlError>`. See [§8](#8-error-handling).

**Progress reporting is push-based over `mpsc` channels, not return values.**
Long-running operations (download, repair, verify, cloud-save transfer, Proton
download) take a `tokio::sync::mpsc::UnboundedSender<EventType>` that you
create; the operation sends status/progress events into it as it works, and
your code drains the paired `UnboundedReceiver` concurrently (typically on
another task). The `async fn` itself only resolves once the whole job is
done (or fails) — the channel is for progress, the `Result` is for the final
outcome. **This structure is unconditional and unchanged across every
operation in this library, including the new pause/resume/cancel feature in
§5.3** — pausing/cancelling a job does not touch its progress channel at all;
it's driven and observed through a separate handle (`DownloadControl`).

**Caching is in-memory, per-`GogDl`-instance, and mostly unbounded.**
Catalog/manifest lookups (`get_game_builds`, `get_downloadable_files`,
`get_product_details`, etc.) are cached indefinitely for the lifetime of the
`GogDl` instance — there is no TTL or invalidation. If you need fresh data,
construct a new `GogDl`. (Exception: `get_owned_games`/`OwnedGames` only
"counts" as cached once the list is non-empty — see [§4](#4-game-catalog).)

**Not everything a struct exposes is a type you can name.** Several public
structs have fields typed with structs from a private submodule that isn't
re-exported at the crate root (e.g. `DownloadableFiles.product_files` is a
`Vec<DepotFile>`, but `DepotFile` itself cannot be written as a type in your
own code). This compiles and works fine via field access, iteration, and
method calls — you just can't spell those inner type names. See
[§9](#9-type-reachability--what-you-can-and-cant-name) for the full list and
how to work with it.

---

## 2. Getting started

```rust
use gogdl_lib2::GogDl;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let http_client = reqwest::Client::new();
    let gog = GogDl::new_from_client(http_client);

    // First run: send the user to gog.get_login_url(), have them log in and
    // paste back the `code` query param from the redirect, then:
    let persisted_auth_json = gog.login_with_code("the-code-from-the-redirect").await?;
    // Save `persisted_auth_json` to disk/keychain/etc.

    // Subsequent runs: restore instead of logging in again.
    // gog.restore_auth(&persisted_auth_json).await?;

    let owned = gog.get_owned_games().await?;
    println!("You own {} games", owned.owned.len());
    Ok(())
}
```

```rust
pub struct GogDl { /* private fields */ }

impl GogDl {
    /// Builds a `GogDl` around a caller-supplied `reqwest::Client` (so you
    /// control TLS/proxy/timeout/user-agent config for the underlying HTTP
    /// stack). No network calls happen here; auth starts unset.
    pub fn new_from_client(client: reqwest::Client) -> Self;
}
```

---

## 3. Authentication

GOG's OAuth-style flow, wrapped down to four methods. The library never
exposes its internal token struct (`Auth`) — you interact with auth state
purely through an **opaque, persistable string**.

```rust
impl GogDl {
    /// The URL to send the user to in a browser/webview to log in. After
    /// login, GOG redirects to `embed.gog.com/on_login_success?...code=...`;
    /// extract the `code` query parameter and pass it to `login_with_code`.
    pub fn get_login_url(&self) -> &str;

    /// Exchanges an OAuth `code` (from the login redirect) for tokens.
    /// Stores them internally for subsequent authenticated calls, AND
    /// returns them serialized as a JSON string for you to persist
    /// (disk, keychain, secure storage — your choice).
    pub async fn login_with_code(&self, code: &str) -> Result<String, GogDlError>;

    /// Restores previously-persisted auth (the string `login_with_code` or
    /// `refresh_auth` returned) into this `GogDl` instance, e.g. on app
    /// startup instead of making the user log in again.
    pub async fn restore_auth(&self, json_str: &str) -> Result<(), GogDlError>;

    /// Uses the stored refresh token to obtain a new access token. Updates
    /// internal state and returns the new state serialized, same as
    /// `login_with_code` — persist the returned string to replace what you
    /// had stored before. Fails with an auth error if no token was ever
    /// restored/set.
    pub async fn refresh_auth(&self) -> Result<String, GogDlError>;
}
```

### Flow

1. **First run:** `get_login_url()` → open in a browser/webview → user logs
   in → GOG redirects with a `?code=...` param → `login_with_code(code)`.
   Persist the returned string.
2. **Every subsequent run:** `restore_auth(&persisted_string)` before calling
   anything else that needs authentication.
3. **When a call fails due to an expired token:** call `refresh_auth()` and
   persist its returned string, then retry. (Note: several internal call
   paths already retry once automatically on an HTTP 401 by refreshing — see
   [§8](#8-error-handling) — but a `refresh_auth()` failure surfaces as
   `GogDlError`, meaning the refresh token itself is no longer valid and the
   user needs to log in again from step 1.)

> **Caveat — automatic refresh is not persisted for you.** Every
> authenticated call already retries once on an HTTP 401 by silently
> refreshing the access token internally (kept in memory on the `GogDl`
> instance) — your call still succeeds without you doing anything. However,
> that fresh token is **not** handed back to you unless you call
> `refresh_auth()` yourself. If your process restarts before you've done
> that, `restore_auth` will restore the *old*, already-rotated-out refresh
> token from disk, which may now be rejected. For a long-lived app, call
> `refresh_auth()` (and re-persist its result) proactively on a schedule —
> e.g. shortly before the `valid_until` timestamp in the persisted JSON — or
> at least after any operation that could have silently refreshed, rather
> than relying purely on reactive 401 handling.

> **Caveat — game/product IDs must be numeric strings.** Some internal calls
> parse the `game_id`/`product_id` string you pass as an integer and
> `.unwrap()` the result (e.g. the ownership check inside
> `get_downloadable_products`, and `get_secure_links`). Passing a non-numeric
> string where a `&str` id is expected will panic rather than return an
> `Err`. GOG product IDs are always numeric in practice, but if you're
> building the id from user input or an external source, validate it's
> parseable as an integer first.

### The persisted string's shape (informational, not a stable contract)

The JSON string is not a documented/versioned wire format of this library —
it's `serde_json::to_string` of an internal struct — but at the time of
writing it contains:

```json
{
  "access_token": "...",
  "refresh_token": "...",
  "expires_in": 3600,
  "token_type": "bearer",
  "session_id": "...",
  "scope": null,
  "user_id": "...",
  "valid_until": 1799999999
}
```

`valid_until` is a Unix timestamp (`expires_in` seconds after the moment the
token was minted) computed and added by this library, purely for your own
convenience if you want to proactively refresh before expiry rather than
waiting for a 401 — it is not required. Treat the whole string as opaque:
store it and hand it back to `restore_auth`, don't build logic that depends
on unlisted fields staying present.

---

## 4. Game catalog

All catalog methods are authenticated (they need `restore_auth`/
`login_with_code` to have been called first) except where noted.

```rust
impl GogDl {
    /// The GOG product IDs the authenticated account owns.
    pub async fn get_owned_games(&self) -> Result<OwnedGames, GogDlError>;

    /// A single game's title (and, structurally, its id — see caveat below).
    pub async fn get_game_details(&self, game_id: i32) -> Result<GameDetails, GogDlError>;

    /// Published builds for a game (Windows OS only — see caveat below).
    pub async fn get_game_builds(&self, game_id: i32) -> Result<GameBuilds, GogDlError>;

    /// Box art / background / Galaxy-background image URLs. Unauthenticated call.
    pub async fn get_game_links(&self, game_id: i32) -> Result<GameLinks, GogDlError>;

    /// The game's store description/summary text. Unauthenticated call.
    pub async fn get_game_summary(&self, game_id: i32) -> Result<GameSummary, GogDlError>;

    /// Raw screenshot link data (see `GameScreenshots::resolve_links` to turn
    /// this into plain URLs). Unauthenticated call.
    pub async fn get_game_screenshots(&self, game_id: i32) -> Result<GameScreenshots, GogDlError>;

    /// Product metadata (title, product type) for any product id — not
    /// restricted to owned games. Unauthenticated call.
    pub async fn get_product_details(&self, product_id: &str) -> Result<ProductDetails, GogDlError>;
}
```

### Types

```rust
pub struct OwnedGames {
    pub owned: Vec<i32>, // GOG product ids
}

pub struct GameDetails {
    pub title: String,
    pub id: i32, // always 0 — not populated by this method; ignore it
}

pub struct GameBuilds {
    pub game_title: String, // always empty string — not populated; ignore it
    pub count: i32,
    pub items: Vec<GameBuild>, // see §9 — GameBuild's type name is not exported
}
// GameBuild's fields, readable via `.items[i].<field>` even though you can't
// name the `GameBuild` type itself:
//   build_id: String
//   version_name: String       <- pass this as `build_name` to download methods
//   date_published: chrono::DateTime<chrono::Utc>
//   link: String                (internal manifest URL; not meant for direct use)

pub struct GameLinks {
    pub links: Links, // §9: `Links`'s type name is not exported
}
// Links { box_art_image: GogImage, background_image: GogImage, galaxy_background_image: GogImage }
// GogImage { href: String }

pub struct GameSummary {
    pub summary: Summary, // §9: `Summary`'s type name is not exported
}
// Summary { default: String, english: String }

pub struct GameScreenshots {
    pub embedded: Embedded, // §9: not exported; use `resolve_links()` instead of poking at this
}
impl GameScreenshots {
    /// Turns the raw (possibly templated) screenshot data into plain image
    /// URLs. Always returns `Ok`; a screenshot without a usable variant is
    /// silently skipped rather than erroring.
    pub fn resolve_links(&self) -> Result<Vec<String>, GamesError>;
}

pub struct ProductDetails { /* private fields */ pub title: String }
impl ProductDetails {
    pub fn get_product_type(&self) -> String; // e.g. "game", "dlc"
}
```

### Non-obvious behavior

- **`get_owned_games` caching quirk:** the result is only treated as cached
  once `owned` is non-empty. An account that genuinely owns zero games will
  hit the network on *every* call — there's no way to negative-cache "zero
  owned games" the way `get_game_details` can (see next point).
- **`get_game_details` negative-caches "not a game":** if the underlying API
  call fails to decode (typically because `product_id` is DLC or some other
  non-game product), that failure is cached, and every subsequent call for
  that id returns `GogDlError::GameError(GamesError::ProductNotAGame)`
  immediately without another network round-trip.
- **`get_game_builds` is Windows-only.** There is no OS parameter; only
  Windows builds are ever returned.
- **`get_game_links` and `get_game_screenshots` hit the same underlying URL**
  (`api.gog.com/v2/games/{id}`) but decode different parts of the response
  and cache independently — calling both for the same game fetches twice.

---

## 5. Download flow

### 5.1 Discovering what to download

```rust
impl GogDl {
    /// Every product (base game + owned DLC) available for the named build,
    /// grouped with their depots. Use this to build a "select which DLC to
    /// install" UI before calling `get_downloadable_files`.
    pub async fn get_downloadable_products(
        &self,
        game_id: i32,
        build_name: &str, // == some GameBuild.version_name from get_game_builds
    ) -> Result<Vec<DownloadableProduct>, GogDlError>;

    /// Resolves the actual file lists (with per-chunk manifests) for the
    /// given products of one build — this is what you actually pass to
    /// `download_files`/`repair_files`/`verify_files`.
    pub async fn get_downloadable_files(
        &self,
        game_id: i32,
        build_name: &str,
        selected_products: &[&str], // product ids, e.g. &[&main_game_id, &dlc_id]
    ) -> Result<Vec<DownloadableFiles>, GogDlError>;
}
```

```rust
pub struct DownloadableProduct {
    pub product_id: String,
    pub depots: Vec<Depot>, // §9: `Depot`'s type name is not exported
}
// Depot's fields, readable via `.depots[i].<field>`:
//   manifest: String
//   size: u64                (uncompressed depot size in bytes)
//   compressed_size: u64
//   product_id: String
//   languages: Vec<String>   ("*" means "all languages")

#[derive(Clone)]
pub struct DownloadableFiles {
    pub product_id: String,
    pub product_files: Vec<DepotFile>, // §9: `DepotFile`'s type name is not exported
    pub is_dependency: bool, // always false today; reserved for a future
                              // dependency/redistributable-depot flow
}
// DepotFile's fields/methods, usable via `.product_files[i].<field>()`:
//   md5: Option<String>
//   sha256: Option<String>
//   path: String                       (relative path within the install dir)
//   chunks: Option<Vec<Chunk>>         (`Chunk` also unnameable; None => 0-byte/placeholder file)
//   file_type: String
//   fn get_file_size(&self) -> u64     (sum of chunk sizes; 0 if `chunks` is None)
//   fn get_download_units(&self) -> Vec<DownloadUnit>  (flattens chunks; empty if `chunks` is None)
```

Typical flow: `get_downloadable_products` → let the user pick which products
(DLC) to install → `get_downloadable_files(game_id, build_name,
&selected_product_ids)` → pass the resulting `Vec<DownloadableFiles>` to
`download_files`.

Both calls are cached indefinitely per `(game_id, build_name, ...)` key for
the life of the `GogDl` instance.

> **Caveat — depots are silently filtered to English only.** Fetching a
> build's metadata (which both `get_downloadable_products` and
> `get_downloadable_files` do internally) keeps only depots whose
> `languages` list contains `"en-US"` or the wildcard `"*"`; every
> other-language depot in the manifest is dropped before you ever see it.
> There is currently no way to select a different language edition through
> this library — if a game ships language-specific depots, only the English
> one (or a language-agnostic one marked `"*"`) is reachable.

### 5.2 Downloading (`download_files`)

```rust
impl GogDl {
    /// Downloads every file in `files` into `path` from scratch (fresh
    /// install — no existing-install diffing, unlike `repair_files`).
    /// Progress and lifecycle events are pushed onto `tx` as the job runs;
    /// this future itself resolves only once the whole job is done (or a
    /// fatal error occurs). `control` drives/observes pause, resume, and
    /// cancel for this job — see §5.3.
    pub async fn download_files(
        &self,
        files: Vec<DownloadableFiles>,
        path: &str,               // install root; created if it doesn't exist
        control: DownloadControl,
        tx: mpsc::UnboundedSender<DownloadJobEvent>,
    ) -> Result<(), GogDlError>;
}
```

```rust
use tokio::sync::mpsc;
use gogdl_lib2::{DownloadControl, DownloadJobEvent, DownloadStage, DownloadDetail, DownloadEvent, FileAllocationEvent};

let control = DownloadControl::new();
let (tx, mut rx) = mpsc::unbounded_channel();

let files = gog.get_downloadable_files(game_id, "1.0", &["12345"]).await?;

let control_for_job = control.clone();
let download_task = tokio::spawn(async move {
    gog.download_files(files, "/games/MyGame", control_for_job, tx).await
});

while let Some(event) = rx.recv().await {
    // `stage` and `detail` are independent fields (not a matched tagged
    // union), but `detail`'s own variant already tells you which one you
    // have — match on it directly rather than pairing it with `stage`.
    match event.detail {
        DownloadDetail::Allocation(a) => match a {
            FileAllocationEvent::FileAllocated(path) => println!("allocated {path}"),
            FileAllocationEvent::FileOk => {}
            _ => { /* handle allocation errors */ }
        },
        DownloadDetail::Download(d) => match d {
            DownloadEvent::Total { bytes, chunks } => println!("job is {bytes} bytes across {chunks} chunks"),
            DownloadEvent::Progress { bytes } => { /* accumulate into a total-downloaded counter */ }
            DownloadEvent::ChunkDownloaded { path } => { /* one chunk finished */ }
            _ => { /* handle per-chunk errors */ }
        },
    }
}

download_task.await.unwrap()?;
```

#### Event types

```rust
pub struct DownloadJobEvent {
    pub stage: DownloadStage,   // Allocating | Downloading
    pub detail: DownloadDetail, // Allocation(FileAllocationEvent) | Download(DownloadEvent)
}

pub enum DownloadStage { Allocating, Downloading }

pub enum DownloadDetail {
    Allocation(FileAllocationEvent),
    Download(DownloadEvent),
}

pub enum FileAllocationEvent {
    PathResolveError(String),  // manifest path couldn't be resolved under `path`
    FileAllocated(String),     // file didn't exist (or was the wrong size); pre-sized on disk now
    AllocationError(String),   // couldn't create/resize the file
    FileSizeError(String),     // couldn't stat the existing file
    FileOk,                    // file already existed at the correct size — nothing to do
}

pub enum DownloadEvent {
    /// Sent once at the start of the download stage: total decoded bytes
    /// and chunk count queued. Track progress as
    /// `downloaded = Σ Progress.bytes`, `remaining = Total.bytes - downloaded`.
    Total { bytes: u64, chunks: usize },
    /// An incremental delta of decoded bytes written for a chunk still in
    /// flight — sent as each network read is decoded and flushed to disk.
    Progress { bytes: u64 },
    /// One chunk finished downloading and writing successfully, in full.
    ChunkDownloaded { path: String },
    SecureLinkError(String),   // no usable CDN link for this chunk's product
    DownloadError(String),     // chunk failed after exhausting retries
    WriteError(String),        // chunk downloaded but couldn't be written to disk
    PathResolveError(String),  // destination path couldn't be resolved
}
```

**How to track overall progress:** sum every `DownloadEvent::Progress.bytes`
you receive; compare against the `Total.bytes` sent once at stage start.
Chunk-level errors (`SecureLinkError`/`DownloadError`/`WriteError`/
`PathResolveError`) are reported per-chunk on the channel; the job's overall
`Result` only becomes an `Err` for setup-level failures (e.g. can't create the
install directory) — a chunk that exhausts retries does **not** stop other
chunks or fail the whole job's `Result`, so if you need "did everything
actually succeed," track chunk error events yourself and compare the count
against `Total.chunks`.

### 5.3 Pause / resume / cancel (`DownloadControl`)

`DownloadControl` is a cheap-to-clone handle for controlling one download or
repair job's lifecycle from outside the task that's awaiting it. You construct
it, pass one clone into `download_files`/`repair_files`, and keep another
clone (or several, e.g. one held by a UI layer) to drive and observe it.
**This is entirely separate from the `mpsc` progress channel** — pausing does
not emit anything on `tx`; you observe pause/cancel state exclusively through
`DownloadControl` itself.

```rust
#[derive(Clone)]
pub struct DownloadControl { /* private */ }

impl DownloadControl {
    /// A fresh handle in the `Running` state.
    pub fn new() -> Self;

    /// Requests a pause. Chunks already in flight finish and flush normally
    /// (drain semantics — nothing is interrupted mid-transfer); no *new*
    /// chunk starts until `resume()`. No-op once cancelled or completed.
    pub fn pause(&self);

    /// Un-pauses: parked chunks proceed. No-op once cancelled or completed.
    pub fn resume(&self);

    /// Requests cancellation. Like pause, chunks already in flight drain to
    /// completion; no new chunk starts. Terminal — pause()/resume() become
    /// no-ops afterward. The job's `download_files`/`repair_files` future
    /// still resolves `Ok(())` once everything has drained (see below).
    /// No-op once the job has completed.
    pub fn cancel(&self);

    /// The current status, computed live from in-flight state.
    pub fn status(&self) -> JobStatus;

    /// `true` once a pause has been requested (`Pausing` or `Paused`).
    pub fn is_paused(&self) -> bool;

    /// `true` once a cancel has been requested (`Cancelling` or `Cancelled`).
    pub fn is_cancelled(&self) -> bool;

    /// A `watch` receiver that immediately yields the current status and
    /// every subsequent transition, with no missed edges. This is how a
    /// frontend shows "Pausing…" transitioning to "Paused" once every
    /// in-flight chunk has actually finished draining.
    pub fn subscribe(&self) -> tokio::sync::watch::Receiver<JobStatus>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobStatus {
    Running,
    Pausing,    // pause requested; one or more chunks still draining
    Paused,     // pause requested; nothing in flight
    Cancelling, // cancel requested; one or more chunks still draining
    Cancelled,  // cancel requested; nothing in flight (terminal)
    Completed,  // job future resolved without a cancel (terminal)
}
```

#### Two-phase transitions

Both pause and cancel are **two-phase**. Calling `pause()` while chunks are
mid-transfer immediately publishes `JobStatus::Pausing`; the moment the last
already-in-flight chunk finishes writing (and no new chunk has started, since
they're parked), the status settles to `JobStatus::Paused`. The same shape
applies to `cancel()` → `Cancelling` → `Cancelled`. If nothing happens to be
in flight at the moment you call `pause()`/`cancel()`, the settled state
(`Paused`/`Cancelled`) is published immediately — there's no artificial delay.

```rust
let control = DownloadControl::new();
let mut status_rx = control.subscribe();

tokio::spawn(async move {
    loop {
        let status = *status_rx.borrow_and_update();
        println!("job status: {:?}", status);
        // Stop on a terminal status — `Completed` (or `Cancelled`) is
        // always published when the job's future resolves, so this task
        // never outlives its job.
        if matches!(status, JobStatus::Completed | JobStatus::Cancelled) {
            break;
        }
        if status_rx.changed().await.is_err() {
            break;
        }
    }
});

// elsewhere, e.g. a "pause" button handler:
control.pause();
// ... later, a "resume" button:
control.resume();
// ... or to stop for good:
control.cancel();
```

#### Semantics you should rely on

- **Drain, never discard:** a chunk that has already started downloading
  always finishes and flushes its bytes to disk before the job pauses or
  cancels — you will never end up with a chunk half-written mid-buffer.
- **Concurrency-bounded drain window:** because chunks are downloaded with
  bounded concurrency (up to `DownloadConfig::max_concurrency`, default 64),
  up to that many chunks may already be in flight the instant you call
  `pause()`; the transition to `Paused` completes once all of them finish —
  typically the time for one chunk's remaining transfer, not the whole job.
- **`cancel()` still resolves the job `Ok(())`.** Once every in-flight chunk
  has drained after a cancel, `download_files`/`repair_files` returns
  `Ok(())`, not an error — cancellation is a normal, successful wind-down, not
  a failure. Check `control.is_cancelled()` (or the terminal `JobStatus`) to
  tell "the user cancelled" apart from "the job actually finished downloading
  everything."
- **Completion is always signalled.** When a `download_files`/`repair_files`
  future resolves — success *or* error — the status settles to a terminal
  `JobStatus::Completed`, unless a cancel was requested first, in which case
  `Cancelled` wins (so `is_cancelled()` keeps telling "user cancelled" apart
  from "actually finished"). A status watcher must exit on `Completed`/
  `Cancelled` (as in the example above); waiting only on `changed()` would
  otherwise park forever once the job is done, leaking the watcher task and
  the control handle.
- **Resume ramp-up:** the adaptive concurrency controller (see
  [§5.6](#56-tuning-network-behavior-downloadconfig)) is aware of pauses and
  re-baselines instead of shrinking to the concurrency floor while paused, so
  resuming doesn't force a slow re-climb.
- **One `DownloadControl` per job.** Don't reuse the same handle across two
  separate `download_files`/`repair_files` calls — construct a new one per
  job (cheap: `DownloadControl::new()`).
- **`verify_files` is not gated.** `DownloadControl` only applies to
  `download_files` and `repair_files`; `verify_files` is a read-only checksum
  pass and has no pause/cancel hook.

### 5.4 Repairing an install (`repair_files`)

```rust
impl GogDl {
    /// Repairs an existing install: verifies files against `files`'
    /// manifests, re-allocates/re-downloads only what's missing or doesn't
    /// match. Same `DownloadControl`/`mpsc` shape as `download_files`.
    pub async fn repair_files(
        &self,
        files: Vec<DownloadableFiles>,
        path: &str,
        control: DownloadControl,
        tx: mpsc::UnboundedSender<RepairEvent>,
    ) -> Result<(), GogDlError>;
}
```

```rust
pub struct RepairEvent {
    pub stage: RepairStage,
    pub detail: RepairDetail,
}

pub enum RepairStage { VerifyingFiles, Allocating, VerifyingChunks, Downloading }

pub enum RepairDetail {
    FileVerify(FileVerifyEvent),
    Allocation(FileAllocationEvent), // same type as in §5.2
    ChunkVerify(VerifyChunksEvent),
    Download(DownloadEvent),         // same type as in §5.2
}

pub enum FileVerifyEvent {
    FileNotFound(String),
    CouldNotResolvePath(String),
    CouldNotReadFileSize(String),
    SizeMismatch(String, u64, u64), // (path, actual_size, expected_size)
    ChecksumMismatch(String),
    FileOk,
}

pub enum VerifyChunksEvent {
    PathResolveError(String),
    FileNotFound(String),
    ChecksumCalculationError(String),
    ChunkChecksumMismatch(String),
}
```

`repair_files` runs four stages in order, all reported on the same `tx`,
tagged by `RepairStage`:

1. **VerifyingFiles** — checks every file's on-disk size against the manifest
   (this stage checks size only, not checksum, despite `ChecksumMismatch`
   existing on `FileVerifyEvent` — files with a mismatched size are flagged
   `SizeMismatch` and queued for re-download).
2. **Allocating** — (re-)allocates files that failed verification.
3. **VerifyingChunks** — for files that passed step 1 (so weren't already
   queued wholesale), verifies each chunk's MD5 individually; mismatching
   chunks are queued for re-download.
4. **Downloading** — downloads every chunk queued by steps 1 and 3, using the
   same `DownloadEvent` shape and adaptive concurrency as a fresh install.

### 5.5 Verifying an install (`verify_files`)

```rust
impl GogDl {
    /// Read-only integrity check: verifies every chunk's MD5 against an
    /// existing install. Does not modify anything on disk and has no
    /// `DownloadControl` (not pausable/cancellable — it's a fast, bounded
    /// checksum pass, not a network-bound download).
    pub async fn verify_files(
        &self,
        files: Vec<DownloadableFiles>,
        path: &str,
        tx: mpsc::UnboundedSender<VerifyEvent>,
    ) -> Result<(), GogDlError>;
}

pub enum VerifyEvent {
    CouldNotResolvePath(String),
    FileNotFound(String),
    ChunkChecksumMismatch(String),
    ChunkOk,
}
```

### 5.6 Tuning network behavior (`DownloadConfig`)

```rust
impl GogDl {
    /// Overrides network tuning for every subsequent `download_files`,
    /// `repair_files`, and `verify_files` call on this `GogDl` instance.
    pub async fn set_download_config(&self, config: DownloadConfig);
}

#[derive(Clone, Debug)]
pub struct DownloadConfig {
    pub min_concurrency: usize,      // default 4  — floor for chunk concurrency
    pub max_concurrency: usize,      // default 64 — ceiling for chunk concurrency
    pub sample_interval: Duration,   // default 750ms — how often concurrency is re-tuned
    pub step: usize,                 // default 4  — permits grown/shrunk per sample
    pub response_timeout: Duration,  // default 30s — deadline for a chunk's initial response
    pub idle_timeout: Duration,      // default 15s — deadline for each body-stream read
    pub max_retries: usize,          // default 5  — attempts per chunk before giving up
    pub base_backoff: Duration,      // default 200ms — base for exponential retry backoff
}
impl Default for DownloadConfig { /* the values above */ }
```

Chunk-download concurrency is **adaptive**, not fixed: a background
controller hill-climbs the number of concurrent chunk downloads between
`min_concurrency` and `max_concurrency` against measured throughput, backing
off immediately on a burst of transient errors (timeouts, connection resets,
5xx/429 responses). You generally don't need to touch this — the defaults are
tuned for typical broadband — but raise `max_concurrency` for a very fast
link, or lower `min_concurrency`/`max_concurrency` and lengthen
`response_timeout`/`idle_timeout` for a slow or flaky one.

---

## 6. Cloud saves

```rust
impl GogDl {
    /// Whether this game's Galaxy client supports cloud saves on this OS,
    /// and if so, where. `client_id` is one half of `get_save_auth_ids`.
    pub async fn get_remote_config(&self, client_id: &str) -> Result<RemoteConfig, GogDlError>;

    /// The (client_id, client_secret) pair used to authenticate cloud-save
    /// calls for this game — pass both into every other save method below.
    pub async fn get_save_auth_ids(&self, game_id: i32) -> Result<(String, String), GogDlError>;

    /// Every save file currently stored in the cloud for this client_id.
    pub async fn get_save_file_list(
        &self,
        client_id: &str,
        client_secret: &str,
    ) -> Result<Vec<SaveFile>, GogDlError>;

    /// Downloads one save file to `path` (decompresses, verifies its MD5 if
    /// the server supplied one, and restores its cloud-stored modified
    /// time). Progress via `tx`.
    pub async fn download_save_file(
        &self,
        save_file: &SaveFile,
        client_id: &str,
        client_secret: &str,
        path: &std::path::Path,
        tx: mpsc::UnboundedSender<SaveProgress>,
    ) -> Result<(), GogDlError>;

    /// Uploads the local file at `path` to the cloud under `saves/{url_path}`
    /// (gzip-compressed in transit). Progress via `tx`.
    pub async fn upload_save_file(
        &self,
        client_id: &str,
        client_secret: &str,
        path: &std::path::Path,
        url_path: &str,
        tx: mpsc::UnboundedSender<SaveProgress>,
    ) -> Result<(), GogDlError>;

    /// Deletes a save file from cloud storage.
    /// ⚠️ Unverified against a live GOG account — this endpoint was
    /// extrapolated from GOG's REST conventions and has not been confirmed
    /// to actually work in production. Treat a successful `Ok(())` cautiously
    /// and verify by re-listing.
    pub async fn delete_save_file(
        &self,
        save_file: &SaveFile,
        client_id: &str,
        client_secret: &str,
    ) -> Result<(), GogDlError>;
}
```

```rust
pub struct RemoteConfig { /* private fields */ }
impl RemoteConfig {
    /// Cloud saves are enabled for Windows and at least one location is configured.
    /// Only the Windows section of the remote config is consulted.
    pub fn is_supported(&self) -> bool;

    /// Resolves the first configured cloud-storage location into
    /// `(known_folder_name, relative_path)`, e.g. `("Documents", "MyGame/saves")`.
    /// `known_folder_name` is one of: "Saved Games", "Documents", "Desktop",
    /// "AppData/Roaming", "AppData/Local", "ProgramData", "Users/Public", or
    /// the sentinel "INSTALLATION_PATH" (meaning: relative to the game's own
    /// install directory, not a Windows known-folder).
    pub fn get_path(&self) -> Result<(String, String), GogDlError>;
}

#[derive(Debug, Clone)]
pub struct SaveFile(/* private */);
impl SaveFile {
    /// The save's path with any leading `"saves/"` prefix stripped.
    pub fn get_path(&self) -> String;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SaveProgress {
    pub transferred: u64,
    pub total: u64, // 0 if the server didn't report a size
}
```

### Flow

```rust
let (client_id, client_secret) = gog.get_save_auth_ids(game_id).await?;
let remote_config = gog.get_remote_config(&client_id).await?;
if !remote_config.is_supported() {
    // this game doesn't use cloud saves
}
let (folder, relative_path) = remote_config.get_path()?;

let saves = gog.get_save_file_list(&client_id, &client_secret).await?;
for save in &saves {
    println!("{}", save.get_path());
}

let (tx, mut rx) = mpsc::unbounded_channel();
let dest = std::path::Path::new("/local/save/slot1.sav");
tokio::spawn(async move { while let Some(p) = rx.recv().await { /* p.transferred / p.total */ } });
gog.download_save_file(&saves[0], &client_id, &client_secret, dest, tx).await?;
```

---

## 7. Proton-GE releases

```rust
impl GogDl {
    /// Lists Proton-GE releases from GitHub, one page at a time (`page` is
    /// 1-indexed). No caching, no pagination metadata — call again with an
    /// empty result to know you've reached the end.
    pub async fn get_proton_releases(&self, page: i32) -> Result<Vec<Release>, GogDlError>;

    /// Downloads and extracts a release's tarball into `path`. Progress via
    /// `tx`. ⚠️ The tarball is downloaded **entirely into memory** before any
    /// checksum verification or extraction — expect the memory footprint to
    /// briefly match the compressed download size (hundreds of MB for a
    /// Proton-GE build). If the release metadata includes a SHA-256 digest,
    /// it's verified before extraction; if not, extraction proceeds
    /// unverified.
    pub async fn download_proton_release(
        &self,
        release: &Release,
        path: &std::path::Path,
        tx: mpsc::UnboundedSender<ProtonProgress>,
    ) -> Result<(), GogDlError>;
}
```

```rust
pub struct Release {
    pub url: String,
    pub tag_name: String, // e.g. "GE-Proton1-1"
    pub assets: Vec<Asset>,
}
impl Release {
    pub fn get_download_link(&self) -> Option<&str>; // None if no x86_64 .tar.gz asset exists
    pub fn get_download_size(&self) -> Option<u64>;
    pub fn get_checksum(&self) -> Option<&str>;       // raw "sha256:<hex>" or None
}

pub struct Asset {
    pub browser_download_url: String,
    pub name: String,
    pub content_type: String,
    pub digest: Option<String>,
    pub size: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProtonProgress {
    pub transferred: u64,
    pub total: u64,
}
```

### Flow

```rust
let mut page = 1;
let mut all_releases = Vec::new();
loop {
    let releases = gog.get_proton_releases(page).await?;
    if releases.is_empty() { break; }
    all_releases.extend(releases);
    page += 1;
}

let release = &all_releases[0];
let (tx, mut rx) = mpsc::unbounded_channel();
tokio::spawn(async move { while let Some(p) = rx.recv().await { /* p.transferred / p.total */ } });
gog.download_proton_release(release, std::path::Path::new("/opt/proton-ge/GE-Proton1-1"), tx).await?;
```

---

## 8. Error handling

Every fallible `GogDl` method returns `Result<T, GogDlError>`:

```rust
#[derive(Debug)] // implements std::error::Error via thiserror
pub enum GogDlError {
    AuthError(/* private inner type */),
    GameError(/* private inner type */),
    DepotError(/* private inner type */),
    SecureLinksError(/* private inner type */),
    DownloadError(/* private inner type */),
    SavesError(/* private inner type */),
    ProtonError(/* private inner type */),
}
```

`GogDlError` implements `std::error::Error` (via `thiserror`), so:

- `err.to_string()` / `format!("{err}")` gives a human-readable message, e.g.
  `"Download error: Http error: <body>, status: 404"`.
- `std::error::Error::source(&err)` walks down into the wrapped inner error
  (and further, since the inner error types themselves wrap `reqwest`/
  `serde_json`/`std::io` errors via `#[from]`), so you can log/inspect a full
  chain without needing to name any of the intermediate types.
- You **can** match on the outer `GogDlError` variant (`AuthError`,
  `DownloadError`, etc.) to branch on *which subsystem* failed — but you
  **cannot** name or destructure the inner type each variant carries (see
  [§9](#9-type-reachability--what-you-can-and-cant-name)). If you need finer
  distinctions than "which subsystem," match on the `Display` string, or file
  an issue/PR to have the specific variant you need re-exported.

For reference, the inner error enums (all variant names, even though the
types themselves aren't nameable) — useful for interpreting `to_string()`
output or writing log-matching logic:

- **Auth errors:** `UrlParseError`, `NetworkError`, `Unauthorized`, `Http { status, body }`, `DecodeError`, `DeflateError`, `StreamError`, `Timeout`.
- **Games errors:** all of the above, plus `NotAuthenticated`, `ProductNotAGame`.
- **Depot errors:** all of the base set, plus `NotAuthenticated`.
- **SecureLinks errors:** all of the base set, plus `NotAuthenticated`, `GamesError`, `ProductNotOwned(String)`.
- **Download errors:** all of the base set, plus `GamesError`, `DepotError`, `BuildNotFound`, `DiskAllocationError`, `SecureLinksError`.
- **Saves errors:** all of the base set (as `Io` instead of `DeflateError`), plus `NotAuthenticated`, `Games`, `Depot`, `Auth`, `BuildNotFound`, `MalformedRemotePath(String)`, `UnknownFolderKey(String)`, `InvalidTimestamp(String)`, `MalformedSaveLine(String)`, `HashMismatch { expected, actual }`.
- **Proton errors:** all of the base set (as `Io` instead of `DeflateError`), plus `NoDownloadAsset`, `HashMismatch { expected, actual }`.

A `Http { status, body }` variant almost always means "the request went
through but GOG's API rejected it" — check `status` via the message string
(401 typically means your session needs `refresh_auth()`; 404 typically means
a bad id/build name; other codes indicate a genuine API-side problem).

---

## 9. Type reachability — what you can and can't name

This crate re-exports a curated, flat set of types at its root (see the full
list in [§10](#10-full-gogdl-method-index) and the code snippets above). A
few public structs have **fields typed with structs that live in a private
submodule and are not themselves re-exported at the root** — most commonly
because those inner structs are manifest/DTO details nobody outside the
library needs to construct or pattern-match on by name.

**What still works fine**, because Rust resolves field access and method
calls by type inference, not by you spelling the type:

```rust
// DownloadableFiles.product_files is Vec<DepotFile>, and `DepotFile` is not
// nameable — but this all compiles and works:
for file in &downloadable_files.product_files {
    println!("{} ({} bytes)", file.path, file.get_file_size());
    for unit in file.get_download_units() {
        println!("  chunk at offset {}", unit.offset);
    }
}
let first_path = downloadable_files.product_files[0].path.clone();
```

**What does not work** — you cannot write the type name in your own code:

```rust
// error[E0433]: failed to resolve: could not find `DepotFile` in `gogdl_lib2`
fn process(file: gogdl_lib2::DepotFile) { ... }
```

Use type inference (`let x = &files.product_files[0];`), iteration
(`for f in &files.product_files`), or a generic/`impl Trait` parameter instead
of naming these types directly.

### The affected types

| Public, root-exported type | Field | Element type (not nameable) |
|---|---|---|
| `DownloadableFiles` | `product_files` | `DepotFile` (itself has a `chunks: Option<Vec<Chunk>>` field — `Chunk` is also not nameable) |
| `DownloadableProduct` | `depots` | `Depot` |
| `GameBuilds` | `items` | `GameBuild` |
| `GameLinks` | `links` | `Links` (module: `games::game_links`) |
| `GameScreenshots` | `embedded` | `Embedded` (use `resolve_links()` instead) |
| `GameSummary` | `summary` | `Summary` |
| `SecureLinks` | `urls` | `UrlFormat` (itself has a `parameters: CdnUrlParams` field) |
| `GogDlError` | (each variant's payload) | `AuthError` / `GamesError` / `DepotError` / `SecureLinksError` / `DownloadError` / `SavesError` / `ProtonError` |

Also entirely private/unreachable, with no public path at all: `AuthManager`,
`DownloadManager`, `DepotManager`, `GamesManager` is reachable... — wait,
`GamesManager` **is** re-exported (see below) but you never need to construct
one yourself, since `GogDl` owns and uses it internally; `SecureLinksManager`,
`SavesManager`, `ProtonManager`, `HttpClient`, and the internal `Auth`/
`SavesAuth` token structs. None of these are things you construct or hold —
`GogDl` is the only object you interact with.

---

## 10. Full `GogDl` method index

| Method | Returns | Section |
|---|---|---|
| `new_from_client(client)` | `Self` | [§2](#2-getting-started) |
| `get_login_url()` | `&str` | [§3](#3-authentication) |
| `login_with_code(code)` | `Result<String, GogDlError>` | [§3](#3-authentication) |
| `restore_auth(json_str)` | `Result<(), GogDlError>` | [§3](#3-authentication) |
| `refresh_auth()` | `Result<String, GogDlError>` | [§3](#3-authentication) |
| `get_owned_games()` | `Result<OwnedGames, GogDlError>` | [§4](#4-game-catalog) |
| `get_game_details(game_id)` | `Result<GameDetails, GogDlError>` | [§4](#4-game-catalog) |
| `get_game_builds(game_id)` | `Result<GameBuilds, GogDlError>` | [§4](#4-game-catalog) |
| `get_game_links(game_id)` | `Result<GameLinks, GogDlError>` | [§4](#4-game-catalog) |
| `get_game_summary(game_id)` | `Result<GameSummary, GogDlError>` | [§4](#4-game-catalog) |
| `get_game_screenshots(game_id)` | `Result<GameScreenshots, GogDlError>` | [§4](#4-game-catalog) |
| `get_product_details(product_id)` | `Result<ProductDetails, GogDlError>` | [§4](#4-game-catalog) |
| `get_secure_links(game_id)` | `Result<SecureLinks, GogDlError>` | internal-facing; see `SecureLinks` in §9 |
| `get_downloadable_products(game_id, build_name)` | `Result<Vec<DownloadableProduct>, GogDlError>` | [§5.1](#51-discovering-what-to-download) |
| `get_downloadable_files(game_id, build_name, products)` | `Result<Vec<DownloadableFiles>, GogDlError>` | [§5.1](#51-discovering-what-to-download) |
| `set_download_config(config)` | `()` | [§5.6](#56-tuning-network-behavior-downloadconfig) |
| `download_files(files, path, control, tx)` | `Result<(), GogDlError>` | [§5.2](#52-downloading-download_files) |
| `repair_files(files, path, control, tx)` | `Result<(), GogDlError>` | [§5.4](#54-repairing-an-install-repair_files) |
| `verify_files(files, path, tx)` | `Result<(), GogDlError>` | [§5.5](#55-verifying-an-install-verify_files) |
| `get_remote_config(client_id)` | `Result<RemoteConfig, GogDlError>` | [§6](#6-cloud-saves) |
| `get_save_auth_ids(game_id)` | `Result<(String, String), GogDlError>` | [§6](#6-cloud-saves) |
| `get_save_file_list(client_id, client_secret)` | `Result<Vec<SaveFile>, GogDlError>` | [§6](#6-cloud-saves) |
| `download_save_file(save_file, client_id, client_secret, path, tx)` | `Result<(), GogDlError>` | [§6](#6-cloud-saves) |
| `upload_save_file(client_id, client_secret, path, url_path, tx)` | `Result<(), GogDlError>` | [§6](#6-cloud-saves) |
| `delete_save_file(save_file, client_id, client_secret)` | `Result<(), GogDlError>` | [§6](#6-cloud-saves) |
| `get_proton_releases(page)` | `Result<Vec<Release>, GogDlError>` | [§7](#7-proton-ge-releases) |
| `download_proton_release(release, path, tx)` | `Result<(), GogDlError>` | [§7](#7-proton-ge-releases) |

### Full list of root-exported types (`use gogdl_lib2::...`)

`GogDl`, `GogDlError`, `Client` (re-exported `reqwest::Client`),
`DownloadConfig`, `DownloadControl`, `JobStatus`, `DownloadableFiles`,
`DownloadableProduct`, `ProductDetails`, `SecureLinks`,
`DownloadJobEvent`, `DownloadStage`, `DownloadDetail`, `DownloadEvent`,
`FileAllocationEvent`, `FileVerifyEvent`, `RepairEvent`, `RepairStage`,
`RepairDetail`, `VerifyChunksEvent`, `VerifyEvent`,
`GameBuilds`, `GameDetails`, `GameLinks`, `GameScreenshots`, `GameSummary`,
`OwnedGames`, `RemoteConfig`, `SaveFile`, `SaveProgress`,
`Asset`, `Release`, `ProtonProgress`.

All of the event types are also grouped under `gogdl_lib2::events::*` if you
prefer importing them from one path.
