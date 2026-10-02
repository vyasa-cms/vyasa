#!/usr/bin/env bash
# Puts the demo back to its seed: database, uploads and search index.
# Meant to run hourly (see README.md). The site is down for a few seconds.
#
# Whatever happens, the app is started again at the end, and two resets
# never overlap.
set -euo pipefail
cd "$(dirname "$0")"
[ -f seed/demo.dump ] && [ -f seed/media.tar ] || { echo "no seed: run ./seed.sh first" >&2; exit 1; }

exec 9> .reset.lock
flock -n 9 || { echo "a reset is already running" >&2; exit 0; }

restart_app() { docker compose up -d --force-recreate app; }
trap restart_app EXIT

docker compose stop app
docker compose up -d --wait db
# A fresh database rather than restoring over the live one: nothing a
# visitor created survives, whatever the dump does or does not mention.
docker compose exec -T db dropdb -U vyasa --if-exists --force vyasa
docker compose exec -T db createdb -U vyasa vyasa
docker compose exec -T db pg_restore -U vyasa -d vyasa --no-owner --exit-on-error /seed/demo.dump

trap - EXIT
# A new container: empty in-memory media and index, and nothing left in
# its writable layer. The index is rebuilt from the database on start.
restart_app
for _ in $(seq 1 60); do
    docker compose exec -T app true 2>/dev/null && break
    sleep 1
done
docker compose exec -T app sh -c 'tar -C /opt/vyasa -xf -' < seed/media.tar
echo "demo reset at $(date -Is)"
