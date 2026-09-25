# CI image for .gitlab-ci.yml: the system packages and the Rust toolchain, so jobs don't hit
# the network for them. Build it on the runner host with ci/build-image.sh (see the header of
# .gitlab-ci.yml).
FROM debian:bookworm-slim

ARG RUST_VERSION

# build-essential/cmake/pkg-config: aws-lc-sys and friends (reqwest's rustls).
RUN apt-get update -qq \
 && apt-get install -qq -y --no-install-recommends \
      git curl ca-certificates \
      build-essential cmake pkg-config \
 && rm -rf /var/lib/apt/lists/*

# The toolchain is the one pinned in rust-toolchain.toml (build-image.sh passes it in). The
# target must match the toml's `targets`.
ENV RUSTUP_HOME=/opt/rustup \
    CARGO_HOME=/opt/cargo
ENV PATH=/opt/cargo/bin:$PATH

RUN test -n "$RUST_VERSION" \
 && curl -sSf https://sh.rustup.rs | sh -s -- -y --no-modify-path --profile minimal \
      --default-toolchain "$RUST_VERSION" -c rustfmt -c clippy -t x86_64-unknown-linux-gnu \
 && echo "$RUST_VERSION" > /opt/rust-version \
 && rm -rf /opt/cargo/registry /opt/cargo/git
