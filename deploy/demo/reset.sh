#!/usr/bin/env bash
# Puts the demo back to its seed: database, uploads and search index.
# Meant to run hourly (see README.md). Takes the site down for a few
# seconds; visitors see the start-up page and then a clean demo.
set -euo pipefail
cd "$(dirname "$0")"
[ -f seed/demo.dump ] || { echo "no seed/demo.dump: run ./seed.sh first" >&2; exit 1; }

docker compose stop app
docker compose up -d db
docker compose exec -T db pg_restore -U vyasa -d vyasa --clean --if-exists --no-owner /seed/demo.dump
docker compose run --rm --no-deps --entrypoint sh -v "$PWD/seed:/seed:ro" app -c \
    'rm -rf /opt/vyasa/media/* /opt/vyasa/index/* && tar -C /opt/vyasa -xf /seed/media.tar'
docker compose up -d app
echo "demo reset at $(date -Is)"
