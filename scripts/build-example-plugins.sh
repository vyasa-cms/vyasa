#!/usr/bin/env bash
# Rebuilds the example plugin components that the test suite loads, the way
# each example's README describes. Build paths are remapped so the binaries
# carry no local home directory.
set -euo pipefail
ROOT="$(git rev-parse --show-toplevel)"
TARGET="$(mktemp -d)"
trap 'rm -rf "$TARGET"' EXIT
export RUSTFLAGS="--remap-path-prefix=$HOME/.cargo=/cargo --remap-path-prefix=$HOME/.rustup=/rustup --remap-path-prefix=$ROOT=/vyasa"
rustup target add wasm32-wasip2 >/dev/null

# build <source dir> <profile> <output file>
build() {
    local src="$1" profile="$2" out="$3" crate flag=()
    crate=$(sed -n 's/^name *= *"\(.*\)"/\1/p' "$src/Cargo.toml" | head -1 | tr - _)
    [ "$profile" = release ] && flag=(--release)
    cargo build -j "${CARGO_BUILD_JOBS:-4}" --manifest-path "$src/Cargo.toml" \
        --target wasm32-wasip2 "${flag[@]}" --target-dir "$TARGET"
    cp "$TARGET/wasm32-wasip2/$profile/$crate.wasm" "$out"
    echo "built $out ($crate, $profile)"
}

EX="$ROOT/plugin-sdk/examples"
# hello is deliberately a base-world (pre-v2) component: tests use it to
# prove old plugins keep loading. Its source is the template as it was then,
# vendored in examples/hello/source with the WIT of that time.
build "$EX/hello/source" debug "$EX/hello/hello-component.wasm"
for name in bookshelf forum-lite storefront; do
    build "$EX/$name" release "$EX/$name/$name-component.wasm"
done
