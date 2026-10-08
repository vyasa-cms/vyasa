#!/usr/bin/env bash
# Runs the full test suite against a throwaway database.
#
# Database tests fail unless VYASA_TEST_DATABASE_URL is set. This starts a
# throwaway Postgres (and S3) for them, runs the suite, and tears both down.
# Each test clones its own database from a migrated template, so the suite
# runs in parallel.
set -euo pipefail

CONTAINER="vyasa-test-pg"
PORT="${VYASA_TEST_PG_PORT:-55433}"
URL="postgres://vyasa:vyasa@127.0.0.1:${PORT}/vyasa_test"

# Object storage, so the S3 media backend is exercised against a real
# server rather than only against its own unit tests. Signing code that has
# never talked to a service is code nobody has tested.
S3_CONTAINER="vyasa-test-s3"
S3_PORT="${VYASA_TEST_S3_PORT:-59000}"
S3_ENDPOINT="http://127.0.0.1:${S3_PORT}"

cleanup() {
    docker rm -f "$CONTAINER" >/dev/null 2>&1 || true
    docker rm -f "$S3_CONTAINER" >/dev/null 2>&1 || true
}
trap cleanup EXIT

echo "==> starting throwaway Postgres on :${PORT}"
cleanup
docker run -d --name "$CONTAINER" \
    -e POSTGRES_USER=vyasa \
    -e POSTGRES_PASSWORD=vyasa \
    -e POSTGRES_DB=vyasa_test \
    -p "${PORT}:5432" \
    postgres:16 >/dev/null

echo -n "==> waiting for it to accept connections"
for _ in $(seq 1 60); do
    if docker exec "$CONTAINER" pg_isready -U vyasa -q 2>/dev/null; then
        echo " ready"
        break
    fi
    echo -n "."
    sleep 1
done

echo "==> starting throwaway object storage on :${S3_PORT}"
docker run -d --name "$S3_CONTAINER" \
    -e MINIO_ROOT_USER=vyasatest \
    -e MINIO_ROOT_PASSWORD=vyasatestsecret \
    -p "${S3_PORT}:9000" \
    bitnamilegacy/minio:latest@sha256:451fe6858cb770cc9d0e77ba811ce287420f781c7c1b806a386f6896471a349c >/dev/null

echo -n "==> waiting for it"
for _ in $(seq 1 60); do
    if curl -fsS -o /dev/null "${S3_ENDPOINT}/minio/health/live" 2>/dev/null; then
        echo " ready"
        break
    fi
    echo -n "."
    sleep 1
done
# One bucket, created with the same client the tests use.
docker exec "$S3_CONTAINER" mc alias set local http://127.0.0.1:9000 \
    vyasatest vyasatestsecret >/dev/null 2>&1 || true
docker exec "$S3_CONTAINER" mc mb --ignore-existing local/vyasa-media >/dev/null 2>&1 || true

export VYASA_TEST_S3_ENDPOINT="$S3_ENDPOINT"
export VYASA_TEST_S3_BUCKET="vyasa-media"
export VYASA_TEST_S3_KEY="vyasatest"
export VYASA_TEST_S3_SECRET="vyasatestsecret"

echo "==> cargo test --workspace"
# `set -e` does not fire here on its own: the exit status that matters is
# cargo's, and an earlier version of this script let a build failure look
# like a green run.
if ! VYASA_TEST_DATABASE_URL="$URL" cargo test --workspace "$@"; then
    echo
    echo "==> FAILED: cargo test did not succeed" >&2
    exit 1
fi

echo
echo "==> skipped tests remaining (should be none):"
SKIPPED=$(VYASA_TEST_DATABASE_URL="$URL" cargo test --workspace -- --nocapture 2>&1 \
    | grep -ci "skipping" || true)
echo "$SKIPPED"
if [ "$SKIPPED" -ne 0 ]; then
    echo "==> FAILED: ${SKIPPED} test(s) still skipped with a database available" >&2
    exit 1
fi
