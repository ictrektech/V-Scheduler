#!/usr/bin/env bash
set -euo pipefail
# Builds a Rust builder image with every Cargo.lock dependency pre-fetched and
# publishes it to SWR. App builds then run `cargo build --offline` against this
# image instead of reaching crates.io from inside Docker.
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPOSITORY="${RUST_BUILDER_REPOSITORY:-swr.cn-southwest-2.myhuaweicloud.com/ictrek/v-scheduler-rust-builder}"
RUST_VERSION="${RUST_VERSION:-1.97}"
CRATES_MIRROR="${RUST_BUILDER_CRATES_MIRROR:-sparse+https://rsproxy.cn/index/}"

log() { echo "[INFO] $*"; }
die() { echo "[ERROR] $*" >&2; exit 1; }

case "$(uname -m)" in
  x86_64) arch=amd64 ;;
  aarch64|arm64) arch=arm64 ;;
  *) die "unsupported architecture: $(uname -m)" ;;
esac
image="${REPOSITORY}:${RUST_VERSION}-${arch}"

context="$(mktemp -d)"
trap 'rm -rf "$context"' EXIT
cp "${ROOT_DIR}/backend/Cargo.toml" "${ROOT_DIR}/backend/Cargo.lock" "$context/"
mkdir -p "$context/src"
printf 'fn main() {}\n' > "$context/src/main.rs"

cat > "$context/Dockerfile" <<EOF
FROM rust:${RUST_VERSION}-bookworm
RUN mkdir -p /usr/local/cargo && printf '%s\\n' \\
  '[source.crates-io]' \\
  'replace-with = "mirror"' \\
  '' \\
  '[source.mirror]' \\
  'registry = "${CRATES_MIRROR}"' \\
  '' \\
  '[net]' \\
  'retry = 5' \\
  > /usr/local/cargo/config.toml
WORKDIR /fetch
COPY Cargo.toml Cargo.lock ./
RUN mkdir -p src && printf 'fn main() {}\\n' > src/main.rs && cargo fetch --locked && cd / && rm -rf /fetch
EOF

log "Building ${image}"
docker build --platform "linux/${arch}" --provenance=false --sbom=false -t "$image" "$context"
docker run --rm --network none "$image" cargo --version
docker push "$image"
log "Published ${image}"
