#!/usr/bin/env bash
# Builds one wasm module per format into npm/build/, then embeds them as npm/src/wasm/<format>.ts.
set -euo pipefail

cd "$(dirname "$0")/.."

mkdir -p npm/build

build() {
  cargo build --release --locked --target wasm32-unknown-unknown -p htp-decompress-wasm --no-default-features --features "$1"
  cp "${CARGO_TARGET_DIR:-$PWD/target}/wasm32-unknown-unknown/release/htp_decompress_wasm.wasm" "npm/build/$1.wasm"
}

build gzip
build xz

# Measured 1.5x faster than opt-level "s", for about 5 KB more gzipped.
CARGO_PROFILE_RELEASE_OPT_LEVEL=3 build zstd

node scripts/embed-wasm.mjs
