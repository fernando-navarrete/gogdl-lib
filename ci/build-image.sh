#!/bin/sh
# Builds the CI image that .gitlab-ci.yml runs its jobs in.
#
# Usage: ci/build-image.sh [--load-into <container>]
#
# Run it on the runner host (thinkcentre.home) from a checkout. The runner shares the host's
# docker through its mounted docker.sock, so a plain build is all that's needed: jobs see the
# image directly. --load-into <container> is only for a runner with its own docker-in-docker
# daemon: it pipes the image in with `docker save | docker exec -i <container> docker load`.
#
# Run it again after bumping the Rust channel in rust-toolchain.toml or changing ci/Dockerfile; the tag changes, so
# .gitlab-ci.yml's `image:` must be updated to the printed name.
set -eu

# Bump whenever ci/Dockerfile changes; never reset on a channel bump.
IMAGE_REVISION=3

cd "$(dirname "$0")/.."

load_into=
case "${1:-}" in
  --load-into)
    load_into="${2:-}"
    [ -n "$load_into" ] || { echo "error: --load-into needs a container" >&2; exit 1; }
    ;;
  "") ;;
  *) echo "usage: $0 [--load-into <container>]" >&2; exit 1 ;;
esac

RUST_VERSION=$(sed -n 's/^channel *= *"\([^"]*\)".*/\1/p' rust-toolchain.toml)
[ -n "$RUST_VERSION" ] || { echo "error: no channel in rust-toolchain.toml" >&2; exit 1; }

IMAGE="gogdl-lib-ci:rust-$RUST_VERSION-r$IMAGE_REVISION"
docker build -t "$IMAGE" --build-arg "RUST_VERSION=$RUST_VERSION" ci/
echo "built $IMAGE"

if [ -n "$load_into" ]; then
  docker save "$IMAGE" | docker exec -i "$load_into" docker load
  echo "loaded $IMAGE into $load_into"
fi
