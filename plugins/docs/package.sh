#!/usr/bin/env bash
# Packs the marketplace package of vyasa-docs from source and prints its
# sha-256 (what the listing's `sha256` must equal).
#
#   VYASA_SIGNING_KEY=<author secret hex> plugins/docs/package.sh
#
# Needs a `vyasa` binary (on PATH, or VYASA_BIN) for `vyasa plugin pack`.
# Never packs the committed component: a package is built from the sources
# it will be reviewed against.
set -euo pipefail
cd "$(dirname "$0")"
ROOT="$(cd ../.. && pwd)"
: "${VYASA_SIGNING_KEY:?set VYASA_SIGNING_KEY to the author secret (vyasa plugin keygen)}"
VYASA="${VYASA_BIN:-vyasa}"
version="$(sed -n 's/^version = "\(.*\)"/\1/p' manifest.toml | head -1)"

export RUSTFLAGS="${RUSTFLAGS:-} --remap-path-prefix=$HOME/.cargo=/cargo --remap-path-prefix=$HOME/.rustup=/rustup --remap-path-prefix=$ROOT=/vyasa"
rustup target add wasm32-wasip2 >/dev/null
TARGET="$(mktemp -d)"
PKG="$(mktemp -d)"
trap 'rm -rf "$TARGET" "$PKG"' EXIT
cargo build -j "${CARGO_BUILD_JOBS:-4}" --release --locked --target wasm32-wasip2 --target-dir "$TARGET"
cp manifest.toml "$PKG/manifest.toml"
cp "$TARGET/wasm32-wasip2/release/vyasa_docs.wasm" "$PKG/plugin.wasm"
out="$PWD/vyasa-docs-${version}.vyplugin"
"$VYASA" plugin pack "$PKG" --out "$out"
sha256sum "$out"
