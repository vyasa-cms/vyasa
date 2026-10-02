#!/usr/bin/env bash
# Runs the README quick start against a published image, the way a stranger
# would: only docker-compose.yml, no source checkout. Fails if the admin is
# not up and the setup token cannot be claimed within five minutes.
#
# Usage: smoke-quickstart.sh [image-tag]     (default: latest)
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
tag="${1:-latest}"
# The compose file publishes :3000. Never run beside a server already there.
if curl -s -o /dev/null --max-time 2 http://127.0.0.1:3000/; then
    echo "something already answers on :3000; refusing to start a second stack there" >&2
    exit 1
fi
dir="$(mktemp -d)"
project="vyasa-smoke-$$"
cp "$ROOT/docker-compose.yml" "$dir/"
cd "$dir"
export VYASA_VERSION="$tag" COMPOSE_PROJECT_NAME="$project"
cleanup() { docker compose down -v --remove-orphans >/dev/null 2>&1 || true; rm -rf "$dir"; }
trap cleanup EXIT

start=$(date +%s)
# --no-build: the point is the published image, not a local build.
docker compose pull app
docker compose run --rm --no-build app migrate
docker compose up -d --no-build

code=000
for _ in $(seq 1 120); do
    code=$(curl -s -o /dev/null -w '%{http_code}' http://127.0.0.1:3000/admin/login || true)
    [ "$code" = 200 ] && break
    sleep 1
done
[ "$code" = 200 ] || { echo "admin did not come up (last status $code)" >&2; docker compose logs app | tail -40 >&2; exit 1; }

token=""
for _ in $(seq 1 30); do
    token=$(docker compose exec -T app cat .run/setup-token 2>/dev/null || true)
    [ -n "$token" ] && break
    sleep 1
done
[ -n "$token" ] || { echo "no setup token in .run/setup-token" >&2; exit 1; }

headers=$(curl -s -o /dev/null -D - -X POST http://127.0.0.1:3000/api/v1/setup/claim \
    -H 'content-type: application/json' -H 'origin: http://127.0.0.1:3000' \
    -d "{\"token\":\"$token\"}")
status=$(head -1 <<< "$headers" | awk '{print $2}')
case "$status" in 2??) ;; *) echo "setup claim answered $status" >&2; exit 1 ;; esac
grep -qi '^set-cookie:' <<< "$headers" || { echo "setup claim set no cookie" >&2; exit 1; }

elapsed=$(( $(date +%s) - start ))
echo "quick start ok in ${elapsed}s (image tag $tag)"
[ "$elapsed" -le 300 ] || { echo "slower than five minutes" >&2; exit 1; }
