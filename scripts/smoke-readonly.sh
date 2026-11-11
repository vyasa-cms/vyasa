#!/usr/bin/env bash
# Boots the image the way a container platform runs it — read-only root
# filesystem, a tmpfs for scratch, PORT and DATABASE_URL, media in object
# storage — and proves: the setup token is printed even when the run
# directory is not writable, setup works through the API, an upload lands
# in the bucket, and search works after a restart that wiped the index.
#
# Usage: smoke-readonly.sh <image-tag>
set -euo pipefail
tag="${1:?image tag}"
port="${SMOKE_PORT:-38600}"
net="vyasa-ro-$$"
B="http://127.0.0.1:$port"
jar="$(mktemp)"
cleanup() {
    docker rm -f "ro-app-$$" "ro-pg-$$" "ro-minio-$$" >/dev/null 2>&1 || true
    docker network rm "$net" >/dev/null 2>&1 || true
    rm -f "$jar"
}
trap cleanup EXIT
fail() {
    echo "$*" >&2
    echo "--- warnings and errors from the app log:" >&2
    docker logs "ro-app-$$" 2>&1 | grep -E '"level":"(ERROR|WARN)"|^vyasa' | tail -20 >&2 || true
    exit 1
}

docker network create "$net" >/dev/null
docker run -d --name "ro-pg-$$" --network "$net" \
    -e POSTGRES_USER=vyasa -e POSTGRES_PASSWORD=vyasa -e POSTGRES_DB=vyasa \
    postgres:17-alpine >/dev/null
# MinIO's own images are no longer publicly pullable; Bitnami's frozen
# legacy image is, ships mc, and creates the bucket itself.
docker run -d --name "ro-minio-$$" --network "$net" \
    -e MINIO_ROOT_USER=minio -e MINIO_ROOT_PASSWORD=minio123 \
    -e MINIO_DEFAULT_BUCKETS=media \
    bitnamilegacy/minio:latest@sha256:451fe6858cb770cc9d0e77ba811ce287420f781c7c1b806a386f6896471a349c >/dev/null
# Over TCP: the image's init-phase server answers the Unix socket and
# then restarts, which is exactly when the app would connect.
for _ in $(seq 30); do docker exec "ro-pg-$$" pg_isready -h 127.0.0.1 -U vyasa -q && break; sleep 1; done
# The image ships mc; no second image to pull.
mc() { docker exec "ro-minio-$$" mc "$@"; }
for _ in $(seq 60); do
    mc alias set m http://127.0.0.1:9000 minio minio123 >/dev/null 2>&1 \
        && mc ls m/media >/dev/null 2>&1 && break
    sleep 1
done
mc ls m/media >/dev/null || { docker logs "ro-minio-$$" 2>&1 | tail -15 >&2; fail "the media bucket never appeared"; }

storage_env="-e VYASA_STORAGE__PROVIDER=s3 -e VYASA_STORAGE__BUCKET=media
    -e VYASA_STORAGE__REGION=auto -e VYASA_STORAGE__ENDPOINT=http://ro-minio-$$:9000
    -e VYASA_STORAGE__ACCESS_KEY_ID=minio -e VYASA_STORAGE__SECRET_ACCESS_KEY=minio123
    -e VYASA_STORAGE__PATH_STYLE=true"

# $1: extra docker-run arguments (one string, word-split on purpose).
run_app() {
    # shellcheck disable=SC2086
    docker run -d --name "ro-app-$$" --network "$net" --read-only --tmpfs /tmp \
        -p "127.0.0.1:$port:3000" \
        -e PORT=3000 -e DATABASE_URL="postgres://vyasa:vyasa@ro-pg-$$:5432/vyasa" \
        -e VYASA_SECRET_KEY=readonly-smoke-secret-key \
        -e VYASA_MEDIA_DIR=/tmp/media -e VYASA_INDEX_DIR=/tmp/index \
        -e VYASA_LOG__FORMAT=json ${1:-} \
        "$tag" >/dev/null
    for _ in $(seq 60); do curl -fsS "$B/healthz" >/dev/null 2>&1 && break; sleep 1; done
    curl -fsS "$B/healthz" >/dev/null 2>&1 || fail "the app never answered /healthz"
    for _ in $(seq 60); do curl -fsS "$B/readyz" >/dev/null 2>&1 && break; sleep 1; done
    curl -fsS "$B/readyz" >/dev/null 2>&1 || fail "the app never became ready"
}

# First boot with the run directory on the read-only root: the token file
# cannot be written, and the token must still reach the log. No storage in
# the environment: media starts on the tmpfs and moves to the bucket from
# the admin API below.
run_app "-e VYASA_RUN_DIR=/opt/vyasa/.run"
token=""
for _ in $(seq 30); do
    token=$(docker logs "ro-app-$$" 2>&1 | grep -oE 'stp_[0-9a-f]+' | head -1 || true)
    [ -n "$token" ] && break
    sleep 1
done
[ -n "$token" ] || fail "no setup token in the log"

post() { curl -fsS -b "$jar" -c "$jar" -o /dev/null -X POST "$B$1" \
    -H 'content-type: application/json' -H "origin: $B" -d "$2" || fail "POST $1 failed"; }
post /api/v1/setup/claim "{\"token\":\"$token\"}"
post /api/v1/setup/account '{"email":"ro@example.com","username":"ro","display_name":"RO","password":"readonly-password-12","timezone":"UTC"}'
post /api/v1/setup/content '{"theme":"blog","sample_content":true}'
echo "setup ok (token from the log, run dir read-only)"

# A 1x1 PNG: the media allow-list is images, audio, video and documents.
png="/tmp/ro-$$.png"
base64 -d > "$png" <<'PNG'
iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==
PNG
upload() {
    local body
    body=$(curl -sS -b "$jar" -H "origin: $B" -w '\n%{http_code}' -F "file=@$1;type=image/png" "$B/api/v1/media")
    case "$(tail -1 <<< "$body")" in 2??) ;; *) fail "upload failed: $body" ;; esac
    head -1 <<< "$body" | grep -oE '"id":"?[0-9]+' | head -1 | grep -oE '[0-9]+'
}
before=$(upload "$png")
[ -n "$before" ] || fail "no id in the upload response"
echo "upload ok on local disk (media $before)"

# Point media at the bucket from the admin API: a probe object is written,
# read and deleted before anything is saved, and the switch applies to the
# running server.
storage_body="{\"provider\":\"s3\",\"bucket\":\"media\",\"region\":\"auto\",\"endpoint\":\"http://ro-minio-$$:9000\",\"path_style\":true,\"access_key_id\":\"minio\",\"secret_access_key\":\"minio123\"}"
settings=$(curl -sS -b "$jar" -H "origin: $B" -H 'content-type: application/json' -X PUT -d "$storage_body" "$B/api/v1/media/storage")
grep -q '"source":"options"' <<< "$settings" || fail "storage settings not saved: $settings"
# A second, different file lands in the bucket; the first still serves.
printf 'x' >> "$png"
after=$(upload "$png")
rm -f "$png"
objects=$(mc ls --recursive m/media | wc -l)
[ "$objects" -gt 0 ] || fail "the upload did not land in the bucket"
curl -fsS -o /dev/null "$B/api/v1/media/$before/raw" || fail "the file uploaded before the switch stopped serving"
echo "upload ok in the bucket (media $after, $objects object(s)); the earlier one still serves"

# Move the first file across and wait for the job.
curl -fsS -b "$jar" -H "origin: $B" -o /dev/null -X POST "$B/api/v1/media/storage/migrate" || fail "migrate did not start"
state=""
for _ in $(seq 60); do
    state=$(curl -fsS -b "$jar" "$B/api/v1/media/storage" | grep -oE '"state":"[a-z]+"' | head -1)
    [ "$state" = '"state":"done"' ] && break
    sleep 1
done
[ "$state" = '"state":"done"' ] || fail "the move did not finish: $state"
echo "move ok (everything in the bucket)"

# A restart with a fresh tmpfs and the bucket set in the environment this
# time (the environment wins over the saved settings): the index is gone
# and must rebuild itself, and both files serve from the bucket.
docker rm -f "ro-app-$$" >/dev/null
run_app "$storage_env"
curl -fsS -o /dev/null "$B/api/v1/media/$before/raw" || fail "the moved file does not serve after the restart"
curl -fsS -o /dev/null "$B/api/v1/media/$after/raw" || fail "the bucket file does not serve after the restart"
n=0
for _ in $(seq 60); do
    n=$(curl -fsS "$B/api/v1/search?q=Welcome" | grep -o '"id"' | wc -l || true)
    [ "$n" -gt 0 ] && break
    sleep 1
done
[ "$n" -gt 0 ] || fail "search found nothing after the restart"
echo "read-only smoke: ok (search answers $n hit(s) from the rebuilt index)"
