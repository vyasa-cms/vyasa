#!/usr/bin/env bash
# Scans exactly what would be published — `git archive HEAD` — for secrets.
# Uses the gitleaks container so nothing needs installing.
set -euo pipefail
ROOT="$(git rev-parse --show-toplevel)"
TREE="$(mktemp -d)"
trap 'rm -rf "$TREE"' EXIT
git -C "$ROOT" archive HEAD | tar -x -C "$TREE"
docker run --rm -v "$TREE:/scan:ro" -v "$ROOT/.gitleaks.toml:/gitleaks.toml:ro" \
    ghcr.io/gitleaks/gitleaks:v8.21.2 dir /scan --config /gitleaks.toml --redact --no-banner -v
