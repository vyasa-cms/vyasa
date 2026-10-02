#!/usr/bin/env bash
# Checks every relative link in tracked Markdown files resolves (offline).
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
git ls-files -z '*.md' | xargs -0 docker run --rm -v "$PWD:/repo:ro" -w /repo \
    lycheeverse/lychee:latest --offline --no-progress
