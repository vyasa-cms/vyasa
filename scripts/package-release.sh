#!/usr/bin/env bash
# Packages a release archive whose layout matches what the server expects:
# it is started from the archive's own directory and reads admin/dist from
# there.
#
# Usage: package-release.sh <version> <target> <binary> <admin-dist-dir> <out-dir>
# Prints the path of the archive it wrote (a .sha256 sits next to it).
set -euo pipefail
[ $# -eq 5 ] || { echo "usage: $0 <version> <target> <binary> <admin-dist-dir> <out-dir>" >&2; exit 2; }
version="$1" target="$2" binary="$3" admin="$4" out="$5"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"

name="vyasa-${version}-${target}"
stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT
mkdir -p "$stage/$name/admin" "$stage/$name/docs" "$out"
install -m 0755 "$binary" "$stage/$name/vyasa"
cp -R "$admin" "$stage/$name/admin/dist"
cp "$ROOT/README.md" "$ROOT/LICENSE-MIT" "$ROOT/LICENSE-APACHE" "$ROOT/CHANGELOG.md" "$stage/$name/"
cp "$ROOT/docs/DEPLOYMENT.md" "$stage/$name/docs/"

out="$(cd "$out" && pwd)"
tar -C "$stage" -czf "$out/$name.tar.gz" "$name"
# sha256sum on Linux, shasum on macOS; same output format either way.
if command -v sha256sum >/dev/null; then sum=(sha256sum); else sum=(shasum -a 256); fi
( cd "$out" && "${sum[@]}" "$name.tar.gz" > "$name.tar.gz.sha256" )
echo "$out/$name.tar.gz"
