#!/usr/bin/env bash
# Sign and pack a .vyplugin package.
#
# Usage: VYASA_SIGNING_KEY=<hex-32-byte-ed25519-seed> ./sign.sh <dir> [out.vyplugin]
#
# <dir> must contain manifest.toml and plugin.wasm. Canonical form signed:
# sha256(manifest.toml) || sha256(plugin.wasm).
#
# Make a key with:  vyasa plugin keygen
# The server only accepts packages signed by a key listed in
# plugin_trusted_keys (VYASA_PLUGIN_TRUSTED_KEYS=<public hex>).
set -euo pipefail

DIR="${1:?usage: sign.sh <dir> [out]}"
OUT="${2:-${DIR%/}.vyplugin}"

# Prefer an installed binary; fall back to the workspace build so the SDK
# works from a checkout without installing anything.
if command -v vyasa >/dev/null 2>&1; then
    VYASA=(vyasa)
elif [ -x "$(dirname "$0")/../target/release/vyasa" ]; then
    VYASA=("$(dirname "$0")/../target/release/vyasa")
else
    VYASA=(cargo run --quiet --manifest-path "$(dirname "$0")/../Cargo.toml" --bin vyasa --)
fi

exec "${VYASA[@]}" plugin pack "$DIR" --out "$OUT"
