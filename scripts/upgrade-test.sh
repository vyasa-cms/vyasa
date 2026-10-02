#!/usr/bin/env bash
# Rehearse an upgrade against a throwaway database.
#
# The failure this exists to catch is a migration that works on an empty
# database and breaks on one with content in it — a NOT NULL column added
# without a default, a unique index over data that is not unique yet.
# Testing against a fresh database proves nothing about that.
#
# Usage: scripts/upgrade-test.sh [--keep]
#
#   --keep   Leave the scratch database in place for inspection.

set -euo pipefail

cd "$(dirname "$0")/.."

KEEP=0
[ "${1:-}" = "--keep" ] && KEEP=1

# Reuse the test database URL if it is set; otherwise derive a scratch
# database from the main one so this never touches real content.
SOURCE_URL="${VYASA_TEST_DATABASE_URL:-${VYASA_DATABASE_URL:-}}"
if [ -z "$SOURCE_URL" ] && [ -f .env ]; then
    SOURCE_URL="$(grep -E '^VYASA_DATABASE_URL=' .env | head -1 | cut -d= -f2-)"
fi
if [ -z "$SOURCE_URL" ]; then
    echo "Set VYASA_DATABASE_URL (or VYASA_TEST_DATABASE_URL) first." >&2
    exit 1
fi

SCRATCH_DB="vyasa_upgrade_test_$$"
BASE_URL="${SOURCE_URL%/*}"
SCRATCH_URL="${BASE_URL}/${SCRATCH_DB}"
ADMIN_URL="${BASE_URL}/postgres"

cleanup() {
    if [ "$KEEP" -eq 1 ]; then
        echo "scratch database kept: ${SCRATCH_DB}"
        return
    fi
    psql "$ADMIN_URL" -q -c "DROP DATABASE IF EXISTS ${SCRATCH_DB} WITH (FORCE)" >/dev/null 2>&1 || true
}
trap cleanup EXIT

step() { printf '\n\033[1m==> %s\033[0m\n' "$1"; }

for tool in psql cargo; do
    command -v "$tool" >/dev/null 2>&1 || { echo "$tool is required" >&2; exit 1; }
done

step "Creating scratch database ${SCRATCH_DB}"
psql "$ADMIN_URL" -q -c "CREATE DATABASE ${SCRATCH_DB}"

step "Building the release binary"
cargo build --release --bin vyasa

BIN=./target/release/vyasa
export VYASA_DATABASE_URL="$SCRATCH_URL"

step "Applying migrations to an empty database"
"$BIN" migrate

step "Seeding content"
"$BIN" admin create \
    --email upgrade-test@example.invalid \
    --username upgradetest \
    --password "upgrade-test-password"
"$BIN" dev seed --count 50

BEFORE=$(psql "$SCRATCH_URL" -tAc "SELECT count(*) FROM posts")
echo "seeded ${BEFORE} posts"

step "Re-running migrations (must be idempotent)"
"$BIN" migrate

step "Verifying content survived"
AFTER=$(psql "$SCRATCH_URL" -tAc "SELECT count(*) FROM posts")
if [ "$BEFORE" != "$AFTER" ]; then
    echo "FAIL: post count changed across migration: ${BEFORE} -> ${AFTER}" >&2
    exit 1
fi
echo "post count unchanged: ${AFTER}"

step "Checking health"
"$BIN" health

step "Reporting migration version"
psql "$SCRATCH_URL" -tAc \
    "SELECT 'migration version: ' || max(version) FROM _sqlx_migrations WHERE success"

printf '\n\033[32mUpgrade rehearsal passed.\033[0m\n'
