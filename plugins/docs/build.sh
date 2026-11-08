#!/usr/bin/env bash
# Builds the vyasa-docs component, or checks that the committed one is current.
#
#   plugins/docs/build.sh           build docs-component.wasm and record its source hash
#   plugins/docs/build.sh --check   exit 1 when the committed component was not built
#                                   from the sources in this tree
#
# The component is a test fixture (crates/api/tests/docs_plugin.rs loads it)
# and a convenience. The marketplace package is always built from source by
# package.sh, never from this file.
set -euo pipefail
cd "$(dirname "$0")"
ROOT="$(cd ../.. && pwd)"

source_hash() {
    {
        find src assets -type f | LC_ALL=C sort
        printf '%s\n' Cargo.toml Cargo.lock manifest.toml ../../crates/plugins/wit/host.wit
    } | xargs sha256sum | sha256sum | cut -d' ' -f1
}

want="$(source_hash)"

if [ "${1:-}" = "--check" ]; then
    have="$(cat docs-component.srchash 2>/dev/null || true)"
    if [ ! -f docs-component.wasm ] || [ "$have" != "$want" ]; then
        echo "plugins/docs: docs-component.wasm was not built from these sources." >&2
        echo "Run plugins/docs/build.sh and commit docs-component.wasm and docs-component.srchash." >&2
        exit 1
    fi
    echo "plugins/docs: the committed component is current"
    exit 0
fi

# Build paths are remapped so the binary carries no local home directory
# (scripts/check-public-tree.sh scans binaries for it).
export RUSTFLAGS="${RUSTFLAGS:-} --remap-path-prefix=$HOME/.cargo=/cargo --remap-path-prefix=$HOME/.rustup=/rustup --remap-path-prefix=$ROOT=/vyasa"
rustup target add wasm32-wasip2 >/dev/null
TARGET="$(mktemp -d)"
trap 'rm -rf "$TARGET"' EXIT
cargo build -j "${CARGO_BUILD_JOBS:-4}" --release --locked --target wasm32-wasip2 --target-dir "$TARGET"
cp "$TARGET/wasm32-wasip2/release/vyasa_docs.wasm" docs-component.wasm
if command -v wasm-tools >/dev/null 2>&1; then
    wasm-tools validate docs-component.wasm
fi
printf '%s\n' "$want" > docs-component.srchash
echo "built docs-component.wasm ($(wc -c < docs-component.wasm) bytes)"
