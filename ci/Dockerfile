# CI image for .gitlab-ci.yml: the system packages and the Rust toolchain, so jobs don't hit
# the network for them. Build it on the runner host with ci/build-image.sh (see the header of
# .gitlab-ci.yml).
FROM debian:bookworm-slim@sha256:7c7b2c966bc9ee8cedfeef67e0e279108992c77681fa595db4a9d65c06ccc587

ARG RUST_VERSION
# rustup-init is pinned by version and sha256. When bumping, take both from
# https://static.rust-lang.org/rustup/archive/<ver>/x86_64-unknown-linux-gnu/rustup-init.sha256
# (the hash stays hardcoded: one fetched from the same host would only prove the transfer).
ARG RUSTUP_VERSION=1.29.1
ARG RUSTUP_SHA256=dda7234360b7f578ca8b0ddcb80145646fa61a67c1720a5abc7051b35c9fcb71

# openssh-client: only the manual `downstream` job uses it (it clones the consumers over ssh).
# build-essential/cmake/pkg-config: aws-lc-sys and friends (reqwest's rustls).
RUN apt-get update -qq \
 && apt-get install -qq -y --no-install-recommends \
      git curl ca-certificates openssh-client \
      build-essential cmake pkg-config \
 && rm -rf /var/lib/apt/lists/*

# The toolchain is the one pinned in rust-toolchain.toml (build-image.sh passes it in). The
# target must match the toml's `targets`.
ENV RUSTUP_HOME=/opt/rustup \
    CARGO_HOME=/opt/cargo
ENV PATH=/opt/cargo/bin:$PATH

RUN test -n "$RUST_VERSION" \
 && curl -sSf -o /tmp/rustup-init \
      "https://static.rust-lang.org/rustup/archive/$RUSTUP_VERSION/x86_64-unknown-linux-gnu/rustup-init" \
 && echo "$RUSTUP_SHA256  /tmp/rustup-init" | sha256sum -c - \
 && chmod +x /tmp/rustup-init \
 && /tmp/rustup-init -y --no-modify-path --profile minimal \
      --default-toolchain "$RUST_VERSION" -c rustfmt -c clippy -t x86_64-unknown-linux-gnu \
 && rm /tmp/rustup-init \
 && echo "$RUST_VERSION" > /opt/rust-version \
 && rm -rf /opt/cargo/registry /opt/cargo/git
