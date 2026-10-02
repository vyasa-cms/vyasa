#!/usr/bin/env bash
# Load test the public routes and the REST list endpoint.
#
# Drives a server that is already running: see perf/README.md for the setup
# that makes the numbers comparable.

set -euo pipefail

BASE="http://localhost:8080"
PROFILE="default"
SLUG="seed-post-1"

usage() {
    cat <<'USAGE'
Usage: perf/load.sh [--base URL] [--profile default|light|heavy] [--slug SLUG]

  --base     Server to test (default: http://localhost:8080)
  --profile  Concurrency profile (default: default)
  --slug     A published post slug to use for the single-post route
USAGE
}

while [ $# -gt 0 ]; do
    case "$1" in
        --base) BASE="$2"; shift 2 ;;
        --profile) PROFILE="$2"; shift 2 ;;
        --slug) SLUG="$2"; shift 2 ;;
        -h|--help) usage; exit 0 ;;
        *) echo "unknown argument: $1" >&2; usage; exit 2 ;;
    esac
done

if ! command -v oha >/dev/null 2>&1; then
    echo "oha is not installed. Install it with:" >&2
    echo "    cargo install oha" >&2
    exit 1
fi

# Concurrency and duration per profile. "light" is for a laptop that is also
# running the database; "heavy" is for finding the saturation point.
case "$PROFILE" in
    light)   CONNECTIONS=10;  DURATION=10s ;;
    default) CONNECTIONS=50;  DURATION=20s ;;
    heavy)   CONNECTIONS=200; DURATION=30s ;;
    *) echo "unknown profile: $PROFILE" >&2; exit 2 ;;
esac

if ! curl -fsS -o /dev/null "$BASE/" 2>/dev/null; then
    echo "cannot reach $BASE — is the server running?" >&2
    exit 1
fi

echo "Vyasa load test"
echo "  target:      $BASE"
echo "  profile:     $PROFILE ($CONNECTIONS connections, $DURATION)"
echo

run() {
    local name="$1" path="$2"
    echo "── $name ($path)"
    # --no-tui keeps the output greppable; -z runs for a duration rather
    # than a fixed request count, so slow routes are not under-sampled.
    oha --no-tui -z "$DURATION" -c "$CONNECTIONS" "$BASE$path" 2>&1 \
        | grep -E "Requests/sec|Success rate|^  50\.00%|^  95\.00%|^  99\.00%" \
        || true
    echo
}

run "home"        "/"
run "single post" "/post/$SLUG"
run "search"      "/search?s=render"
run "feed"        "/feed.xml"
run "rest list"   "/api/v1/posts"

echo "Cache effectiveness is on /metrics (admin session required):"
echo "  vyasa_render_cache_hits_total / (hits + misses)"
