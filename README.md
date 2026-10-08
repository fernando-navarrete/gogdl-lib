# gogdl-lib

Rust client for GOG's Galaxy backend: authentication, catalog browsing, downloading, repairing and
verifying game installs, cloud saves, and Proton-GE releases.

`GogDl` is the only entry point: build one with `GogDl::new_from_client` and call its methods. Every
other public type is data you get back or hand in. The rustdoc is the spec (`cargo doc --open`).

## Using it

Pin by git tag, never a branch:

```toml
gogdl-lib = { git = "https://github.com/fernando-navarrete/gogdl-lib", tag = "v1.2.3" }
```

A minor release may change a signature or behavior; a patch never does. `ROADMAP.md` has the
versioning rules and `CHANGELOG.md` the changes per release.

## This repository is a mirror

Development happens on a self-hosted GitLab. This GitHub repository is a read-only push mirror of
`main` and the `v*` tags: issues, pull requests and Actions are off here. Report a problem to the
maintainer directly (see `SECURITY.md`).

## License

Licensed under either of [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
