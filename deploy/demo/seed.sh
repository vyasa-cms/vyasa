#!/usr/bin/env bash
# Builds the demo's seed: a fresh database with the shared account and
# sample content, dumped to seed/demo.dump, plus the uploaded media in
# seed/media.tar. Run once, and again after upgrading the demo image.
#
# The app runs with demo mode OFF while seeding (demo mode refuses some of
# the setup it needs, such as the site address), then the stack is stopped.
set -euo pipefail
cd "$(dirname "$0")"
EMAIL="${DEMO_EMAIL:-demo@vyasa.site}"
PASSWORD="${DEMO_PASSWORD:-vyasademo}"
SITE="${DEMO_SITE_URL:-https://demo.vyasa.site}"
PORT="${DEMO_PORT:-38400}"
BASE="http://127.0.0.1:$PORT"
mkdir -p seed

docker compose down -v --remove-orphans
docker compose up -d db
docker compose run --rm --no-deps app migrate
docker compose run --rm --no-deps app admin create --email "$EMAIL" --password "$PASSWORD" --username demo
# Twelve published posts so lists, archives and search have something in them.
docker compose run --rm --no-deps app dev seed --count 12
VYASA_DEMO__ENABLED=false docker compose up -d app

for _ in $(seq 1 90); do
    [ "$(curl -s -o /dev/null -w '%{http_code}' "$BASE/admin/login" || true)" = 200 ] && break
    sleep 1
done

jar="$(mktemp)"; trap 'rm -f "$jar"' EXIT
api() { # method path [json]
    curl -sf -b "$jar" -c "$jar" -H 'content-type: application/json' -H "origin: $BASE" \
        -X "$1" "$BASE/api/v1$2" ${3:+-d "$3"} > /dev/null
}
api POST /auth/login "{\"email\":\"$EMAIL\",\"password\":\"$PASSWORD\"}"
api PUT /options/site_url "\"$SITE\""
api PUT /options/site_title '"Vyasa demo"'
api PUT /options/site_tagline '"Try the editor, themes and content types. Everything resets every hour."'

# A custom content type with fields, to show the content model.
api POST /content-types '{"slug":"project","singular":"Project","plural":"Projects","description":"Work samples","public":true}'
api POST /content-types/project/fields '{"key":"client","label":"Client","kind":"text"}'
api POST /content-types/project/fields '{"key":"year","label":"Year","kind":"number"}'
api POST /content-types/project/fields '{"key":"site","label":"Website","kind":"url"}'
doc() { printf '{"schema_version":1,"blocks":[{"kind":"paragraph","attrs":{"text":"%s"}}]}' "$1"; }
for p in "Harbour redesign|Northwind|2025|https://example.com" "Field notes app|Contoso|2024|https://example.org"; do
    IFS='|' read -r title client year url <<< "$p"
    api POST /posts "{\"type\":\"project\",\"title\":\"$title\",\"status\":\"published\",\"content\":$(doc "A sample project."),\"fields\":{\"client\":\"$client\",\"year\":$year,\"site\":\"$url\"}}"
done
api POST /posts "{\"type\":\"page\",\"title\":\"About this demo\",\"status\":\"published\",\"content\":$(doc "This site is a Vyasa demo. Sign in at /admin with the account shown at the top of the page. Everything you change is reset every hour.")}"

docker compose stop app
docker compose exec -T db pg_dump -U vyasa -Fc vyasa > seed/demo.dump
docker compose run --rm --no-deps --entrypoint sh app -c 'tar -C /opt/vyasa -cf - media' > seed/media.tar
docker compose down
echo "seed written: $(du -h seed/demo.dump | cut -f1) database, $(du -h seed/media.tar | cut -f1) media"
