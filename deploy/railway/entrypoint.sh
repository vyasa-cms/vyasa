#!/bin/sh
# Platforms mount their volumes owned by root; the image runs as uid 10001.
# Started as root, this makes the data directories ours and drops to the
# image's user before serving. Started as any other user, it just serves.
set -e
if [ "$(id -u)" = 0 ]; then
    for dir in "${VYASA_INDEX_DIR:-/data/index}" "${VYASA_MEDIA_DIR:-/data/media}"; do
        mkdir -p "$dir"
        chown -R 10001:10001 "$dir"
    done
    exec setpriv --reuid=10001 --regid=10001 --clear-groups vyasa "$@"
fi
exec vyasa "$@"
