#!/usr/bin/env bash
# Tests for scripts/package-release.sh and scripts/release-notes.sh.
set -euo pipefail
ROOT="$(git rev-parse --show-toplevel)"
T="$(mktemp -d)"
trap 'rm -rf "$T"' EXIT
fail() { echo "FAIL: $*" >&2; exit 1; }

# --- package-release.sh ---------------------------------------------------
printf '#!/bin/sh\necho vyasa\n' > "$T/vyasa-bin"
mkdir -p "$T/admin-dist/assets"
echo '<html></html>' > "$T/admin-dist/index.html"
echo 'x' > "$T/admin-dist/assets/app.js"
archive=$("$ROOT/scripts/package-release.sh" 1.2.3 x86_64-unknown-linux-gnu "$T/vyasa-bin" "$T/admin-dist" "$T/out")
[ "$archive" = "$T/out/vyasa-1.2.3-x86_64-unknown-linux-gnu.tar.gz" ] || fail "archive path: $archive"
( cd "$T/out" && sha256sum -c vyasa-1.2.3-x86_64-unknown-linux-gnu.tar.gz.sha256 >/dev/null ) || fail "checksum"
tar -xzf "$archive" -C "$T"
R="$T/vyasa-1.2.3-x86_64-unknown-linux-gnu"
[ -x "$R/vyasa" ] || fail "binary missing or not executable"
[ -f "$R/admin/dist/index.html" ] || fail "admin/dist/index.html missing (server reads admin/dist)"
[ -f "$R/admin/dist/assets/app.js" ] || fail "admin assets missing"
for f in README.md LICENSE-MIT LICENSE-APACHE CHANGELOG.md docs/DEPLOYMENT.md; do
    [ -f "$R/$f" ] || fail "$f missing"
done
echo "package-release: ok"

# --- release-notes.sh -----------------------------------------------------
cat > "$T/CHANGELOG.md" <<'MD'
# Changelog

## [Unreleased]

## [0.2.0] - 2026-11-01

## [0.1.0] - 2026-10-10

First public release.

- One thing.

## [0.0.9] - 2026-09-01

Old.
MD
out=$(CHANGELOG_FILE="$T/CHANGELOG.md" "$ROOT/scripts/release-notes.sh" 0.1.0)
expected=$'First public release.\n\n- One thing.'
[ "$out" = "$expected" ] || fail "notes body: [$out]"
if CHANGELOG_FILE="$T/CHANGELOG.md" "$ROOT/scripts/release-notes.sh" 9.9.9 >/dev/null 2>&1; then fail "missing version accepted"; fi
if CHANGELOG_FILE="$T/CHANGELOG.md" "$ROOT/scripts/release-notes.sh" 0.2.0 >/dev/null 2>&1; then fail "empty section accepted"; fi
echo "release-notes: ok"

# --- check-release-version.sh ---------------------------------------------
cat > "$T/Cargo.toml" <<'TOML'
[workspace.package]
version = "0.1.0-rc.1"
TOML
CARGO_TOML="$T/Cargo.toml" "$ROOT/scripts/check-release-version.sh" 0.1.0-rc.1 >/dev/null || fail "matching version refused"
if CARGO_TOML="$T/Cargo.toml" "$ROOT/scripts/check-release-version.sh" 0.1.0 >/dev/null 2>&1; then fail "mismatched version accepted"; fi
echo "check-release-version: ok"
