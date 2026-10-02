#!/usr/bin/env bash
# Publish gate: fails when the committed tree (HEAD) contains personal
# strings, links to internal-only docs, or files that must never be public.
# Scans HEAD's content, not the working tree, binaries included (git grep -a):
# compiled fixtures embed build paths. File names are scanned too.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

SELF='scripts/check-public-tree.sh'
PERSONAL='cppprograms|gopal|krishiv|/home/|gopal-XPS|076bc862'
INTERNAL='docs/phases/|docs/superpowers/|STATUS\.md|HANDOVER\.md|PRODUCT-SITES\.md|deploy-tunnel\.sh'
FORBIDDEN='(^|/)\.env$|\.dump$|\.vyplugin$|(^|/)plugin-signing-key|\.pem$|(^|/)\.opencode/|(^|/)deploy-tunnel\.sh$'
# Deliberate public mentions: "path:fixed text" lines that may match.
ALLOW='^GOVERNANCE\.md:.*\(\[@techgopal\]\(https://github\.com/techgopal\)\)'

status=0

files=$(git ls-tree -r --name-only HEAD)
forbidden=$(grep -E "$FORBIDDEN" <<< "$files" || true)
named=$(grep -i -E "$PERSONAL" <<< "$files" || true)
if [ -n "$forbidden$named" ]; then
    echo "files that must not be public:"
    printf '%s\n%s\n' "$forbidden" "$named" | sed '/^$/d; s/^/  /'
    status=1
fi

scan() { # $1 label, $2 pattern
    local hits
    hits=$(git grep -a -n -i -I -E "$2" HEAD -- . ":(exclude)$SELF" 2>/dev/null \
        | sed 's/^HEAD://' | grep -v -E "$ALLOW" || true)
    # Binaries: git grep -I skips them, so scan those separately.
    local bin
    bin=$(git grep -a -l -i -E "$2" HEAD -- . ":(exclude)$SELF" 2>/dev/null \
        | sed 's/^HEAD://' | while IFS= read -r f; do
            git grep -I -q -i -E "$2" HEAD -- "$f" 2>/dev/null || echo "$f (binary)"
        done || true)
    if [ -n "$hits$bin" ]; then
        echo "$1:"
        if [ -n "$hits" ]; then cut -c1-160 <<< "$hits" | sed 's/^/  /'; fi
        if [ -n "$bin" ]; then sed 's/^/  /' <<< "$bin"; fi
        status=1
    fi
}
scan "personal strings" "$PERSONAL"
scan "links to internal-only docs" "$INTERNAL"

[ "$status" -eq 0 ] && echo "public tree clean"
exit "$status"
