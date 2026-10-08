# Security policy

This file covers the supply chain: what a new crate has to meet, how to keep a dev machine safe while
building here, and what to do when a crate we use is reported compromised. The checks that enforce most
of it are in `.gitlab-ci.yml` and described in `CLAUDE.md` ("Gates").

This is a single-maintainer project, published as a read-only GitHub mirror of a self-hosted GitLab.
Report a problem to the maintainer directly, not in a public issue (issues are off on the mirror).

## Adding a crate

A new crate has to meet all of these:

- **Needed.** It isn't replaceable by a few lines of our own code.
- **Maintained.** Recent releases, issues answered, and more than one maintainer or a known
  organisation where possible.
- **At least 7 days old**, the same wait Renovate applies. Cargo has no `--before`: pick the version on
  the crate's crates.io versions page, add it with `cargo add <crate>@=<version>` (relax the `=` once
  it is in), and never run a bare `cargo update` to get it.
- **Few transitive crates.** Check `cargo tree -e normal -i <crate>` and read the `Cargo.lock` diff;
  prefer the smaller option, and `default-features = false` where the crate allows it.

In the MR, list the new crates (direct and transitive) and skim the lockfile diff; the MR template has
a checkbox for it. CI then runs everything with `--locked`, `audit` (`cargo deny`: RustSec advisories,
yanked crates, crates.io as the only source, the license allowlist in `deny.toml`), `scan` (OSV-Scanner
fails on any `MAL-` advisory) and `secrets` (gitleaks).

### Build-time code

Cargo has no `ignore-scripts`: a `build.rs` or a proc macro runs code on every machine and CI job that
builds the crate, and 66 crates in the graph have one today. Say in the MR which new crates do. List
them with:

```sh
cargo metadata --locked --format-version 1 \
  | jq -r '.packages[] | select(any(.targets[].kind[]; . == "custom-build" or . == "proc-macro")) | .name'
```

## Dev machines

- Keep no long-lived tokens in the shell environment while building here: `GITLAB_TOKEN`,
  `GITHUB_TOKEN`, cloud keys such as `AWS_*`. Quick check: `env | grep -iE 'token|secret|key'`.
- Build and test with `--locked`. Never `cargo install` without `--locked`. Run `cargo update` only on
  purpose and with `-p <crate>`: a plain one skips the 7-day wait for every transitive crate.

## If a crate we use is reported compromised

1. **Find it.** `cargo tree -i <crate>`, and grep `Cargo.lock` for the bad name and versions on `main`
   and on open branches.
2. **Did a build run it?** Look at the job logs of every cargo job (`lint`, `test`, `doc`, `audit`,
   `downstream`, and `renovate`) since the bad version was published, and at local dates
   (`ls -l --time-style=full-iso ~/.cargo/registry/src/*/<crate>-<version>`).
3. **Rotate secrets** if it could have run, for everything the machine or job could reach:
   `RENOVATE_TOKEN` and `GITHUB_COM_TOKEN`, the GitHub mirror token (in GitLab's mirroring settings), `DOWNSTREAM_DEPLOY_KEY_B64`, and local tokens and SSH keys. The job token expires
   with the job.
4. **Pin a known-good version** with `cargo update -p <crate> --precise <version>`, through an MR as
   usual.
5. **Scan.** Play the weekly scan by hand (CI/CD, Schedules, "Weekly scan", Play) and check `audit` and
   `scan`.
6. **Release only if needed.** Cut a patch only if a floor in `Cargo.toml` had to change. Consumers
   (`gogdl_flutter`, `lumen-cli`) resolve their own lockfiles, so tell them either way.
