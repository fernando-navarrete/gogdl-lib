# CI/CD and supply chain

Work plan to bring this repo's CI/CD up to what `../metatrader-dashboard` runs (its `.gitlab-ci.yml`,
`renovate.json`, `SECURITY.md`, and devlog phases 1–3), adapted to a Rust library, plus a push mirror to
GitHub. It ends with `main` protected: no direct pushes, every change through a merge request whose
pipeline must pass.

Each step is one branch and one small MR into `main`. Until step 12 the protection isn't enforced, so
"everything through an MR" is a discipline from step 1 on, not a setting. Tick each checkbox as it's
done. Writing the devlog and deleting this plan is step 13.

| Step | Theme | MR | Status |
|---|---|---|---|
| **0** | Decisions | (none) | ✅ |
| **1** | Pipelines on MRs and `main`, MR template | `ci/mr-pipelines` | 🟦 |
| **2** | Pre-commit hook | `ci/pre-commit` | ✅ |
| **3** | Pin every image by digest, check the rustup installer | `ci/pin-images` | 🟦 |
| **4** | `cargo-deny`: advisories, sources, licenses (`audit` job) | `ci/cargo-deny` | ✅ |
| **5** | OSV-Scanner on `Cargo.lock` (`scan` job), weekly scan schedule | `ci/osv-scan` | ✅ |
| **6** | Secret scan (`secrets` job), full-history audit | `ci/secret-scan` | ⬜ |
| **7** | `SECURITY.md` | `docs/security-policy` | ⬜ |
| **8** | `renovate.json` and its validation | `ci/renovate-config` | ⬜ |
| **9** | Renovate job and schedule | `ci/renovate-job` | ⬜ |
| **10** | Release flow for a protected `main` | `ci/release-flow` | ⬜ |
| **11** | GitHub mirror | `ci/github-mirror` | ⬜ |
| **12** | Protect `main` and the `v*` tags | `ci/protect-main` | ⬜ |
| **13** | Close-out | `docs/ci-devlog` | ⬜ |

Status: ⬜ not started · 🟦 in progress · ✅ done · ⏸ blocked

Every MR: the four gates in `CLAUDE.md` pass locally, the MR pipeline is green, and the MR is merged with
the method chosen in step 0. None of these steps changes the crate's code or public API, so none of them
is a release; `Cargo.toml`'s `version` stays at `1.2.3` until `v1.3.0`.

## Starting point (2026-10-08)

- `main` at `d2f8d9b`, newest tag `v1.2.3`, toolchain `1.98.1`, CI image `gogdl-lib-ci:rust-1.98.1`.
  Remote: `ssh://git@thinkcentre.home:2200/gogdl/gogdl-lib.git`. No GitHub remote.
- CI (`.gitlab-ci.yml`) runs **only on tags and manual web runs** (to spare the runner): `lint` (fmt +
  clippy), `test`, `doc`, then `release` on `vX.Y.Z` tags, plus the manual `downstream` job. No MR
  pipelines, no security jobs, no schedules, no Renovate.
- The `release` job uses `release-cli:latest`; the CI image is `FROM debian:bookworm-slim` and pipes
  `sh.rustup.rs` into `sh`. Nothing is pinned by digest.
- Releases are tagged on the working branch and fast-forwarded into `main` afterwards (`CLAUDE.md`
  "Releasing", `ROADMAP.md` "Versioning rules").
- `Cargo.lock`: 243 packages, all from crates.io. `Cargo.toml` has no `license`, `repository` or
  `publish` field. There is no `README.md` or `LICENSE`.

### How metatrader-dashboard maps to Rust

| metatrader-dashboard | here |
|---|---|
| `verify`: format, ESLint, svelte-check, tests, build | `lint`, `test`, `doc` (exist; they move to MR pipelines) |
| `.githooks/pre-commit` (Prettier + ESLint on staged files) | `.githooks/pre-commit` (`cargo fmt --check`) |
| `tools/check-lockfile-integrity.mjs`, `lockfile-lint` | `cargo-deny` `[sources]`: crates.io only, no git deps; `--locked` everywhere |
| `npm audit --audit-level=high` | `cargo-deny` `[advisories]` (RustSec) |
| `npm audit signatures` | no crates.io equivalent; not replicated |
| `scan`: OSV-Scanner, fails on `MAL-` | same, on `Cargo.lock` |
| `ignore-scripts=true` | no equivalent (`build.rs` and proc macros always run); covered by the review bar in `SECURITY.md` |
| Renovate: weekly, 7-day `minimumReleaseAge`, grouped, majors off, security fixes skip the wait | same, `cargo` + `gitlabci` + `dockerfile` managers |
| Images pinned `tag@sha256` | same, plus a checksum on `rustup-init` |
| `SCHEDULE=renovate` / `SCHEDULE=scan` schedules, tokens scoped to a `renovate` environment | same |
| `release` from the `CHANGELOG.md` section | exists (`tool/release_notes.sh`) |
| `build-image` (Kaniko) | none: this is a library, nothing to ship but the tag |
| (none) | `secrets` job (gitleaks) and the GitHub push mirror |

---

## Step 0: Decisions

Answer these before step 1; the steps below assume the recommended answer and say where another one
changes them.

- [x] **D1. Merge method.** **Decided: fast-forward merge with squash required** ("Squash commits when
      merging: Require"), source branch deleted, squash commit message = the MR title. Each MR lands as
      exactly one commit on `main`, so the one-commit-per-phase history holds as long as one step = one
      MR. Devlogs cite the squashed SHA on `main`, never branch SHAs (those disappear). A release MR is
      titled `vX.Y.Z: ...`, so its squashed commit keeps today's commit-message convention.
- [x] **D2. Where tags go.** *Recommended: on `main`, after the release MR merges.* Today tags go on the
      working branch and `main` catches up by `--ff-only`. With a protected `main`, any other MR (a weekly
      Renovate one, say) landing first forces a rebase of the branch, and a tag already cut on it is then
      no longer in `main`'s history. A consumer that needs unreleased work tests it with the manual
      `downstream` job, or pins `rev =` on its own feature branch. Each `v1.x.y` becomes one MR (bump,
      changelog, GAPS tick) followed by a tag on the merged commit. Rewrites `CLAUDE.md` "Releasing"
      and `ROADMAP.md` "Versioning rules" (step 10).
      **Decided:** as recommended.
- [x] **D3. Runner load.** MR pipelines replace "tags and web only". *Recommended: accept it,* with
      `interruptible: true` and auto-cancel of redundant pipelines, and keep the three compile jobs
      sharing the cargo registry cache. Each push costs three builds (`lint`, `test`, `doc`); measure the
      step 1 MR and revisit if the runner can't keep up (`doc` could fold into `lint`).
      **Decided:** as recommended.
- [x] **D4. Rust toolchain updates.** Renovate can bump `rust-toolchain.toml` (a custom regex manager),
      but the CI image is built by hand on the runner host, so that MR stays red on the `.toolchain`
      check until someone runs `ci/build-image.sh` and pushes the new `image:` onto the Renovate branch.
      *Recommended: let Renovate open it, alone and never grouped:* the red pipeline is the reminder, and
      new clippy lints land in their own MR.
      **Decided:** as recommended.
- [x] **D5. Range strategy.** Consumers pin this crate by tag and resolve their own lockfiles, so this
      repo's `Cargo.lock` only decides what CI tests against, while a raised floor in `Cargo.toml` forces
      every consumer up. *Recommended: `rangeStrategy: "update-lockfile"`:* Renovate MRs only touch
      `Cargo.lock`, never need a tag, and never reach consumers. A floor is raised by hand, in a
      release, when the code needs it.
      **Decided:** as recommended.
- [x] **D6. Mirror visibility and license.** *Recommended: a public GitHub repo*, which needs a
      `LICENSE`, a `license` field and a short `README.md` (step 11), and the full-history secret audit
      (step 6) to pass first. The GOG client id/secret in `src/constants/` are the public Galaxy client
      values every open-source GOG client ships; confirm that's what they are. A private mirror skips
      the license and README items, but the history audit stays.
      **Decided:** public, `MIT OR Apache-2.0`; the Galaxy client id/secret in `src/constants/mod.rs` are
      the public values (checked 2026-10-08).
- [x] **D7. How the mirror pushes.** *Recommended: GitLab's built-in push mirror* (Settings →
      Repository → Mirroring repositories, "Mirror only protected branches"), with a fine-grained GitHub
      token (`Contents: read and write`, that one repo, with an expiry). The token lives in the mirror
      settings, not in a CI variable, so no job (and no `build.rs` running in one) can read it. The
      alternative, a `mirror` CI job on `main` and tags, would expose the token to the pipeline.
      **Decided:** as recommended.
- [x] **D8. Lint scope.** *Recommended: keep `lint` as is* (fmt + `clippy -D warnings`) and add
      `cargo deny check` as its own job (step 4). No `clippy::pedantic`, no `shellcheck` for now.
      **Decided:** as recommended.

**Done when:** each answer is written here and the steps below match it.

---

## Step 1: Pipelines on MRs and `main`, MR template

Branch `ci/mr-pipelines`. This plan is the branch's first commit.

- [x] `workflow:rules` as in metatrader-dashboard: `schedule`, `merge_request_event`, branch pipelines
      skipped when the branch has an open MR, `main`, `vX.Y.Z` tags; keep `web` (manual runs).
- [x] `default: interruptible: true`; every job gets `rules` that skip it on `schedule` (the scheduled
      jobs come in steps 5 and 9). `downstream` stays manual and `allow_failure: true`, so it never
      blocks an MR, and is excluded from schedules.
- [x] Rewrite the header comment of `.gitlab-ci.yml` (pipelines now run on MRs and `main`).
- [x] `.gitlab/merge_request_templates/Default.md`: Summary; checklist: the four gates pass locally;
      `CHANGELOG.md` line if consumers see a change; `GogDl` table in `CLAUDE.md` updated if a method
      changed; `Cargo.lock` changed → new crates listed (the bar comes in step 7).
- [ ] Project settings: merge method per D1 (fast-forward merge, squash required, squash message from the MR title), "Delete source branch" by default, "Auto-cancel redundant
      pipelines" on, "Pipelines must succeed" on (enforced from now on, before `main` is protected).
- [x] `CLAUDE.md` "Gates": pipelines run on MRs, `main` and tags.

**Done when:** this MR's own pipeline ran `lint`, `test`, `doc` (and offered `downstream`), and was
merged through the MR; a pushed branch without an MR runs nothing.

## Step 2: Pre-commit hook

Branch `ci/pre-commit`.

- [x] `.githooks/pre-commit`, no dependency: `cargo fmt --check` when a staged file is `*.rs`. Checks,
      never rewrites (a hook that formats and re-stages can commit half of a partly staged file). No
      clippy or tests (CI runs those); `git commit --no-verify` skips it.
- [x] `CLAUDE.md`: `git config core.hooksPath .githooks` once per clone.

**Done when:** a commit with a misformatted `.rs` file is refused locally.

## Step 3: Pin every image by digest, check the rustup installer

Branch `ci/pin-images`.

- [x] `release`: `release-cli:latest` → `release-cli:vX.Y.Z@sha256:…` (release-cli publishes a single-arch manifest, not an index; its digest is pinned).
- [x] `ci/Dockerfile`: `debian:bookworm-slim@sha256:…`.
- [x] `ci/Dockerfile`: replace `curl https://sh.rustup.rs | sh` with a pinned `rustup-init` version
      downloaded from `static.rust-lang.org/rustup/archive/<ver>/…` and checked against its published
      `.sha256` before it runs.
- [x] The image tag gains a revision (`gogdl-lib-ci:rust-1.98.1-r2`), since the image now changes without
      the channel changing. `ci/build-image.sh` reads it from one place, and the `.toolchain` check
      keeps comparing the channel only.
- [ ] Rebuild on the runner host, update `image:`.

**Done when:** no `image:` or `FROM` without `@sha256`, and the MR pipeline is green on the rebuilt image.

## Step 4: `cargo-deny` (`audit` job)

Branch `ci/cargo-deny`.

- [x] `cargo-deny` at a pinned version, at least 7 days old, installed in `ci/Dockerfile` with
      `cargo install --locked` (another image revision, rebuilt as in step 3), so the job installs nothing.
- [x] `deny.toml`:
  - `[advisories]`: RustSec database; `yanked = "deny"`; unmaintained and unsound reported. Any
        accepted advisory goes in `ignore` with a reason and a GAPS entry (as metatrader's P-30).
  - `[sources]`: `unknown-registry = "deny"`, `unknown-git = "deny"`, crates.io only. This is the
        Rust equivalent of the lockfile integrity check (`Cargo.lock` already carries a checksum per
        crate, and every job runs `--locked`).
  - `[licenses]`: an allowlist matching what the 243 crates use today (`cargo deny list` to start);
        this crate's own license, `MIT OR Apache-2.0` (D6).
  - `[bans]`: `multiple-versions = "warn"`, nothing denied yet.
- [x] `audit` job, stage `check`, runs on MRs, `main`, tags, and the `SCHEDULE=scan` schedule (step 5):
      `cargo deny --locked check`.
- [x] `license = "MIT OR Apache-2.0"` in `Cargo.toml`, `LICENSE-MIT` and `LICENSE-APACHE` (moved here from
      step 11, so `deny.toml` needs no exception for this crate).
- [x] First run found RUSTSEC-2026-0285 in `rustls 0.23.41`; fixed with `cargo update -p rustls --precise
      0.23.45` (also moves `rustls-webpki`, `aws-lc-rs`, `aws-lc-sys`, and adds `pkg-config`).
- [x] Rebuild the image on the runner host (`ci/build-image.sh`, now `-r3`).
- [x] Bite test in a throwaway MR: restore `main`'s `Cargo.lock` (`git checkout main -- Cargo.lock`, rustls
      0.23.41) and `audit` goes red on RUSTSEC-2026-0285. (`ring` is no good for this: cargo-deny checks
      only crates in the build graph, and no enabled feature pulls `ring` in.) Close the MR unmerged.

**Done when:** `audit` is green on `main` and red on the bite test.

## Step 5: OSV-Scanner (`scan` job), weekly scan schedule

Branch `ci/osv-scan`.

- [x] `scan` job: the OSV-Scanner image pinned by digest, entrypoint cleared, no cargo:
      `/osv-scanner scan --lockfile Cargo.lock --format json --output-file osv-scanner.json`. Exit 0
      clean, 1 findings, anything else fails the job. Fail only on a `MAL-` id; CVE/GHSA/RUSTSEC ids are
      printed but gated by `audit` (one failing gate per finding, as in metatrader). Keep the JSON as a
      30-day artifact.
- [x] Schedule "Weekly scan" on `main`, variable `SCHEDULE=scan` and nothing else: runs `audit` and `scan`
      only. A schedule with no or another `SCHEDULE` value runs nothing.
- [x] Bite test in a throwaway MR: a lockfile entry for a crate with a `MAL-` advisory turns `scan` red.

**Done when:** `scan` is green on `main`, red on the bite test, and the schedule played by hand runs
exactly `audit` and `scan`.

## Step 6: Secret scan (`secrets` job), full-history audit

Branch `ci/secret-scan`. Must be done before step 11: a mirror publishes the whole history.

- [ ] Run gitleaks over the whole history locally (`gitleaks git --log-opts="--all"`). Review each
      finding: the fixtures in `tests/fixtures/` are scrubbed captures and the Galaxy client id/secret are
      public (D6). A real leak means rotating the secret and deciding whether to rewrite history before
      any mirror exists (tags would move, so consumers would need re-pinning: stop and decide then).
- [ ] `.gitleaks.toml`: the default rules, plus an allowlist for the reviewed false positives, each with
      a comment saying why.
- [ ] `secrets` job, gitleaks image pinned by digest: on MRs, scans the MR's commits
      (`--log-opts="$CI_MERGE_REQUEST_DIFF_BASE_SHA..HEAD"`); on `main` and tags, the whole history.
- [ ] Bite test in a throwaway MR: a commit with a fake `ghp_…` token turns `secrets` red.

**Done when:** the full-history scan is clean (or every finding is allowlisted with a reason) and
`secrets` is red on the bite test.

## Step 7: `SECURITY.md`

Branch `docs/security-policy`. Modelled on metatrader-dashboard's.

- [ ] **Adding a crate:** needed (not a few lines of our own); maintained; at least 7 days old (pick the
      version on crates.io's versions page; cargo has no `--before`); few transitive crates
      (`cargo tree -e normal -i <crate>`, and read the `Cargo.lock` diff); prefer `default-features =
      false`. A crate with a `build.rs` or a proc macro runs code at build time on every machine and CI
      job, and there is no `ignore-scripts` for cargo: say in the MR which new crates have one.
- [ ] **Dev machines:** no long-lived tokens in the environment while building (`GITLAB_TOKEN`,
      `GITHUB_TOKEN`, cloud keys); `cargo build --locked`; never `cargo install` without `--locked`.
- [ ] **If a crate we use is reported compromised:** find it (`cargo tree -i`, grep `Cargo.lock` on `main`
      and open branches); did a build run it (CI logs since the bad version, local `~/.cargo/registry`
      dates); rotate what it could reach (`RENOVATE_TOKEN`, `GITHUB_COM_TOKEN`, the mirror token, the
      `DOWNSTREAM_DEPLOY_KEY_B64` deploy key, local SSH keys); pin a good version with
      `cargo update -p <crate> --precise`; play the weekly scan; cut a patch only if a floor in
      `Cargo.toml` had to change (consumers resolve their own lockfiles, so tell them either way).
- [ ] MR template: the dependency checkbox points at `SECURITY.md`.
- [ ] `CLAUDE.md` "Layout": `SECURITY.md`, `deny.toml`, `.gitleaks.toml`, `.githooks/`, `renovate.json`.

**Done when:** merged, and the template links it.

## Step 8: `renovate.json` and its validation

Branch `ci/renovate-config`. Config only; nothing runs Renovate yet.

- [ ] `renovate.json`, from metatrader-dashboard's:
  - `extends: ["config:recommended"]`, `enabledManagers: ["cargo", "gitlabci", "dockerfile", "custom.regex"]`.
  - `schedule: ["before 6am on monday"]`, `timezone: "America/Mexico_City"`.
  - `minimumReleaseAge: "7 days"`, `internalChecksFilter: "strict"`.
  - `vulnerabilityAlerts: { enabled: true, minimumReleaseAge: null }`, `osvVulnerabilityAlerts: true`.
  - `rangeStrategy: "update-lockfile"` for `cargo` (D5).
  - `ignoreDeps: ["gogdl-lib-ci"]`: the local CI image isn't in any registry.
  - `pinDigests: true` for `gitlabci` and `dockerfile`.
  - Majors disabled, and **0.x minors treated as majors** (`matchCurrentVersion: "<1.0.0"`,
        `matchUpdateTypes: ["minor"]`, disabled): `sysinfo` 0.39, `rand` 0.10, `async-compression`
        0.4 and friends break on a minor.
  - One grouped MR for minor, patch and digest updates.
  - Custom regex manager for `rust-toolchain.toml`'s `channel`, its own MR, never grouped (D4).
  - `lockFileMaintenance` off: it runs a plain `cargo update`, which bypasses the 7-day age check
        for every transitive crate.
- [ ] `renovate-config` job: `renovate-config-validator --strict renovate.json` in the Renovate image
      (pinned by digest), only when `renovate.json` changes (`rules: changes:`).

**Done when:** the validator passes in the MR pipeline.

## Step 9: Renovate job and schedule

Branch `ci/renovate-job`.

- [ ] `renovate` job copied from metatrader-dashboard: image pinned by digest, `environment: { name:
      renovate, action: access }`, `RENOVATE_PLATFORM=gitlab`, `RENOVATE_ENDPOINT=$CI_API_V4_URL`,
      `RENOVATE_REPOSITORIES=$CI_PROJECT_PATH`, onboarding off, config required, fails when
      `RENOVATE_TOKEN` is missing, `NODE_EXTRA_CA_CERTS`/`GIT_SSL_CAINFO` from `CI_SERVER_TLS_CA_FILE`.
      Only on `SCHEDULE=renovate`.
- [ ] Cargo inside Renovate: the `cargo` manager needs `cargo` to update `Cargo.lock`. Use the image
      that ships tools, or `binarySource: install`; either way `rust-toolchain.toml` makes rustup want
      `1.98.1`, so check in the dry run that it resolves without a long download on the slow network.
- [ ] Variables (protected, masked, environment scope `renovate`): `RENOVATE_TOKEN` (project access
      token, `api` + `write_repository`, role Developer) and `GITHUB_COM_TOKEN` (fine-grained, no
      permissions, for changelog lookups and the rate limit). Calendar reminders before both expire.
- [ ] Schedule "Renovate" on `main`, `SCHEDULE=renovate`, weekly.
- [ ] Dry run first: play the schedule with `RENOVATE_DRY_RUN=full` added to the schedule's variables,
      read the log (pins proposed, nothing younger than 7 days, the 0.x rule holding back the right
      crates), then remove the variable.
- [ ] Bite test: a throwaway `check` job on a branch prints `${RENOVATE_TOKEN:+set}`: it must print
      nothing.

**Done when:** a live run opened the grouped MR (and the digest-pin MR), its pipeline ran every check
job, and it merged through the MR like any other.

## Step 10: Release flow for a protected `main`

Branch `ci/release-flow`. Per D1 and D2: the release MR is titled `vX.Y.Z: ...` and the tag goes on its
squashed commit on `main`.

- [ ] `CLAUDE.md` "Releasing": (1) a release branch with the bump, `Cargo.lock`, the `CHANGELOG.md`
      section and the GAPS tick; (2) MR, green, merged; (3) tag `vX.Y.Z` on the merged commit of `main`
      and push the tag; (4) the tag pipeline's `release` job is green. Drop "on the working branch (not
      `main`)".
- [ ] `ROADMAP.md` "Versioning rules": the same order; consumers needing unreleased work use
      `downstream` or a `rev =` pin on their own feature branch.
- [ ] `release` job: `needs: [lint, test, doc, audit, scan, secrets]`, and a rule that the tagged commit
      is on `main` (`git merge-base --is-ancestor "$CI_COMMIT_SHA" origin/main`, fetching `main` first),
      so a tag cut on a branch can't produce a release.
- [ ] Check `tool/release_notes.sh v1.2.3` still passes locally and the pinned release image starts.
      The job itself is first exercised by `v1.3.0`.

**Done when:** merged; the next release follows it.

## Step 11: GitHub mirror

Branch `ci/github-mirror` (the files below; the mirror itself is a setting). Needs step 6.

- [ ] `repository` (the GitLab URL stays canonical) and
      `description` in `Cargo.toml`; `README.md`: what the crate is, that `GogDl` is the entry point, that
      GitHub is a read-only mirror of the self-hosted GitLab, and how consumers pin it by tag. No crate
      code change, so no release.
- [ ] GitHub: create the repo (empty, no README), description says "mirror". Disable Issues, Projects,
      Wiki and Actions (nobody should be able to add a workflow that runs there). Protect `main` and
      `v*` tags against deletion there too.
- [ ] GitHub fine-grained token (D7), `Contents: read and write` on that repo only, with an expiry and a
      calendar reminder.
- [ ] GitLab: Settings → Repository → Mirroring repositories: push, `https://github.com/<owner>/gogdl-lib.git`,
      password auth with the token, "Mirror only protected branches" on (`main` and the protected `v*`
      tags from step 12; until then `main` is the only protected branch), "Keep divergent refs" off.
- [ ] "Update now", then check on GitHub: `main` at the same SHA, every `v*` tag present, no other
      branches.
- [ ] `CLAUDE.md` "Consumers and pinning": the mirror exists, consumers keep pinning the GitLab URL.
      `SECURITY.md`: the mirror token in the rotation list (if step 7 didn't name it yet).

**Done when:** a merge into `main` shows up on GitHub within minutes, and a feature branch never does.

## Step 12: Protect `main` and the `v*` tags

Branch `ci/protect-main` (docs only; the rest is settings).

- [ ] Protected branch `main`: allowed to push and merge "No one" / "Maintainers", force push off,
      code-owner approval off (single maintainer).
- [ ] Merge request settings: "Pipelines must succeed" (on since step 1), "All threads must be
      resolved", "Skipped pipelines are considered successful" off, merge method per D1 (fast-forward, squash required).
- [ ] Protected tags `v*`: create allowed to Maintainers only. Consumers pin by tag, so a moved or
      re-created tag is a supply-chain change for them: nobody but a maintainer can create, and nobody
      can update or delete without unprotecting first.
- [ ] `CLAUDE.md`: "`main` is protected: changes go through a merge request (fast-forward with squash, pipelines must
      succeed)", as metatrader-dashboard's says.
- [ ] Bite tests: a direct `git push origin main` is rejected; an MR with a failing test can't be merged;
      `git push --delete origin v1.2.3` is rejected; the mirror still updates after a merge.

**Done when:** all four bite tests behave as expected.

## Step 13: Close-out

Branch `docs/ci-devlog`: the first MR merged under the protection.

- [ ] `devlog/v1.2.x-ci-cd.md` (decisions, what was built, gotchas, verification, left for later), in the
      shape of the existing devlogs, citing squashed SHAs on `main` (D1).
- [ ] GAPS: any accepted advisory or open item from these steps; `CLAUDE.md` "Layout" points at the new
      devlog.
- [ ] Delete this plan.

**Done when:** merged through an MR with a green pipeline, and mirrored to GitHub.
