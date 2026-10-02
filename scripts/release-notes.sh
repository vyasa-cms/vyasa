#!/usr/bin/env bash
# Prints the CHANGELOG section for one version (without its heading), for
# use as release notes. Fails when the section is missing or empty, so a
# release never goes out with no notes.
#
# Usage: release-notes.sh <version>      (CHANGELOG_FILE overrides the path)
set -euo pipefail
[ $# -eq 1 ] || { echo "usage: $0 <version>" >&2; exit 2; }
version="$1"
file="${CHANGELOG_FILE:-$(cd "$(dirname "$0")/.." && pwd)/CHANGELOG.md}"

body=$(awk -v v="$version" '
    index($0, "## [" v "]") == 1 { found = 1; next }
    found && /^## \[/ { exit }
    found { print }
' "$file" | sed -e '/./,$!d' | sed -e ':a' -e '/^\n*$/{$d;N;ba' -e '}')

if ! grep -q "^## \[$version\]" "$file"; then
    echo "release-notes: no section for $version in $file" >&2; exit 1
fi
if [ -z "$(tr -d '[:space:]' <<< "$body")" ]; then
    echo "release-notes: the section for $version in $file is empty" >&2; exit 1
fi
printf '%s\n' "$body"
