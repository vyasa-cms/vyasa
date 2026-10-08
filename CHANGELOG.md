# Changelog

All notable changes are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project
follows [semantic versioning](https://semver.org) as described in
[docs/VERSIONING.md](docs/VERSIONING.md).

## [Unreleased]

### Added

- Media storage is configurable from the admin: Settings → Delivery →
  Media storage takes an S3-compatible bucket (R2, S3, MinIO), tests the
  connection, and switches new uploads to it without a restart. The keys
  are sealed with `VYASA_SECRET_KEY`; `VYASA_STORAGE__*` in the
  environment still wins and makes the page read-only. Files uploaded
  before a switch keep serving from where they are; "Move existing
  files" copies them across as a resumable background job. New routes:
  `GET`/`PUT /api/v1/media/storage`, `POST /api/v1/media/storage/test`,
  `POST /api/v1/media/storage/migrate`.
- The Cloudflare package passes the R2 keys only when both secrets are
  set, so a site can start on the container's disk and move to R2 from
  the admin.

## [0.2.0] - 2026-10-08

Vyasa runs well on container platforms. Cloudflare Containers, Fly.io,
Railway, Render and Kubernetes get a working site from the official image
and environment variables alone; see "Container platforms" in
[docs/DEPLOYMENT.md](docs/DEPLOYMENT.md#container-platforms).

### Added

- `PORT` is honoured when `VYASA_BIND_ADDR` is unset, and `DATABASE_URL`
  when `VYASA_DATABASE_URL` is unset; Vyasa's own names always win.
- `VYASA_ADMIN_EMAIL` and `VYASA_ADMIN_PASSWORD` create the first
  administrator at boot when the users table is empty, exactly as
  `vyasa admin create` does, and mark setup done. Users already present
  or only one variable set is a warning; a weak password stops the boot.
- `GET /healthz` (liveness) and `GET /readyz` (readiness: database
  reachable, no migration pending) outside `/api/v1`, unauthenticated.
- `run_dir` / `VYASA_RUN_DIR` (default `.run`) for the setup token; the
  image sets `/tmp/vyasa-run`, so it runs with a
  read-only root filesystem and a tmpfs on `/tmp`. `scripts/smoke-readonly.sh`
  boots the image that way in CI, with media in object storage.
- `deploy/cloudflare`, `deploy/fly`, `deploy/kubernetes`, `deploy/railway`
  and `deploy/render`: one page and config per platform. Cloudflare and
  Fly were run live before this release, Kubernetes on a kind cluster;
  Railway and Render are templates following the same contract.
- `vyasa update apply` had its first real run, 0.1.0 → 0.2.0.

### Changed

- The image no longer declares `VOLUME`s for media and the index; mount
  volumes there yourself (the compose file does) or use object storage.
  The setup token inside the container is at `/tmp/vyasa-run/setup-token`.
- The docs now say what the code has done since 0.1.0: `serve` applies
  pending migrations at boot under the migrator's lock.

## [0.1.0] - 2026-10-05

The first public release: an AI-native content management system in one
server binary plus PostgreSQL. Content is stored as structured blocks,
themes are data compiled to CSS with sandboxed templates, plugins are
WebAssembly components that can only do what they declare, and every
install browses the official marketplace and update channel out of the
box. Linux (x86_64, arm64) and macOS (arm64) archives, a multi-arch
container image, and the quick start in the README.

Since the second release candidate:

- `curl -fsSL https://vyasa.site/install.sh | sh` installs the release
  for your platform (checksum verified) into `~/vyasa` and puts `vyasa`
  on your `PATH`; the one source is `install.sh` in this repository.
- The website moved to its own repository, [vyasa-cms/website](https://github.com/vyasa-cms/website); it still builds its docs from `docs/` here.
- Testing a site address (`/setup/verify-url` and the setup wizard's site
  step) only fetches public addresses, through the same guard as link
  checks and plugin fetches. A loopback or private address is reported as
  local and untested (`local`, `site_url_local`) instead of unreachable.
- Dependencies moved to their current majors: sqlx 0.9, wasmtime 49,
  zip 8, ed25519-dalek 3 and html5ever 0.40; the admin builds with vite 8,
  vitest 5 and TypeScript 6.
- The official marketplace (marketplace.vyasa.site) and update channel
  (updates.vyasa.site) are built in, trusted through keys compiled into
  the binary. The `registry_url`, `registry_trusted_keys`,
  `update_channel_url` and `update_trusted_keys` options and the setup
  wizard's updates step are gone; operators can mirror or switch either
  source off in `vyasa.toml`. **Upgrade note:** stored values of those
  options are deleted (migration 52).
- A theme carrying script must be signed to be uploaded by hand.
  `vyasa theme pack` builds and signs one. `plugin_trusted_keys` is now
  `package_trusted_keys` (the old name still works).
- Marketplace plugins are checked against the author key in their
  listing, so sites no longer need each author's key.

## [0.1.0-rc.2] - 2026-10-03

Second release candidate.

- Demo mode (`VYASA_DEMO__ENABLED`) for running a public sandbox, and a
  ready-made demo stack in `deploy/demo/`.
- The project website and documentation in `website/`.
- Upload endpoints describe their file field in the API reference.
- The release smoke test pulls every image it needs.

## [0.1.0-rc.1] - 2026-10-02

First release candidate of the first public release: Linux and macOS
archives, a container image and the quick start, for testing before 0.1.0.

