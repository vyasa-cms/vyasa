#!/usr/bin/env bash
# Fails unless <version> (a tag without the leading v) is the workspace
# version in Cargo.toml: the binary reports CARGO_PKG_VERSION, and the
# updater compares it with release versions, so they must agree.
#
# Usage: check-release-version.sh <version>   (CARGO_TOML overrides the path)
set -euo pipefail
[ $# -eq 1 ] || { echo "usage: $0 <version>" >&2; exit 2; }
file="${CARGO_TOML:-$(cd "$(dirname "$0")/.." && pwd)/Cargo.toml}"
crate=$(awk '/^\[workspace.package\]/{w=1; next} /^\[/{w=0} w && /^version *=/{gsub(/.*= *"|".*/, ""); print; exit}' "$file")
if [ "$crate" != "$1" ]; then
    echo "tag version $1 does not match the workspace version $crate in $file" >&2
    exit 1
fi
echo "version $1 matches Cargo.toml"
