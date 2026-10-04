# Changelog

All notable changes are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project
follows [semantic versioning](https://semver.org) as described in
[docs/VERSIONING.md](docs/VERSIONING.md).

## [Unreleased]

- The website moved to its own repository, [vyasa-cms/website](https://github.com/vyasa-cms/website); it still builds its docs from `docs/` here.
- Testing a site address (`/setup/verify-url` and the setup wizard's site
  step) only fetches public addresses, through the same guard as link
  checks and plugin fetches. A loopback or private address is reported as
  local and untested (`local`, `site_url_local`) instead of unreachable.
- Dependencies moved to their current majors: sqlx 0.9, wasmtime 49,
  zip 8, ed25519-dalek 3 and html5ever 0.40; the admin builds with vite 8,
  vitest 5 and TypeScript 6.

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

## [0.1.0] - unreleased

First public release.
