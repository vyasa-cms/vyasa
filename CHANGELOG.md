# Changelog

All notable changes are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project
follows [semantic versioning](https://semver.org) as described in
[docs/VERSIONING.md](docs/VERSIONING.md).

## [Unreleased]

## [0.3.5] - 2026-10-24

### Changed

- Search runs on tantivy 0.27; existing indexes are read as they are.
- Feeds are parsed with feed-rs 3.
- `docs/theme-token-schema.json` is JSON Schema 2020-12 (`$defs`, plain
  `$ref`) instead of draft-07, from schemars 1.

### Security

- zod 4.6.7 in the admin build tooling (memory exhaustion on oversized
  input; build-time only).

## [0.3.4] - 2026-10-22

### Added

- The `latest-posts` section takes `"paginate": true`: on a page that lists
  posts, `?page=2` shows the next `count` and the list links to newer and
  older pages. A blog index page is no longer stuck at 20 posts.

### Fixed

- A post card with no author shows only its date, without a leading ` · `.
- Logs carry colour codes only when a terminal reads them; under systemd
  they arrived as `ESC[32m` noise in the journal and in log viewers.
- Docs plugin 0.1.3: after an upgrade that changes how pages are imported,
  the next sync rewrites them even when the repository has no new commit.
  0.1.2's changes reached existing sites only with the next push.

### Security

- dompurify 3.4.17 in the admin build (GHSA-cg2g-xh64-v4q7,
  GHSA-mrv2-3pqr-j5jr); tokio 1.54.1 and ctutils 0.4.5
  (RUSTSEC-2026-0385, RUSTSEC-2026-0386).

## [0.3.3] - 2026-10-21

### Fixed

- Site search leaves out entries marked `noindex`, as search engines do.
- Indexed text is the words a reader sees: inline markup (`<code>`, links,
  `&amp;`) no longer shows in search snippets.
- Docs plugin 0.1.2: a hidden folder hides everything in it, and hidden pages
  are marked `noindex`, so they stay out of the sidebar, the pager and
  search while keeping their addresses. The next sync rewrites every page.

## [0.3.2] - 2026-10-21

### Fixed

- A plugin installed or enabled on a running site has its scheduled tasks at
  once. Before, they were only recorded at the next restart, so the Docs
  plugin installed from the marketplace never synced until then. Disabling a
  plugin drops its tasks.
- Docs plugin 0.1.1: documentation reads well on a phone. The layout keeps a
  single gutter whatever the theme's section padding, wide tables and code
  scroll inside their box, long inline code wraps, an empty version banner
  leaves no gap, and the order is search, pages, contents, then the page.
  Each page shows its title as a heading, and the page drawer is labelled
  "Pages".

### Security

- rustls-webpki 0.103.16 and webpki-roots / webpki-root-certs 1.0.10
  (RUSTSEC-2026-0381, RUSTSEC-2026-0382, RUSTSEC-2026-0383); vite 8.3.4 in
  the admin build (launch-editor advisories).

## [0.3.1] - 2026-10-20

### Security

- A new marketplace signing key is compiled in as the live key; the two
  earlier keys stay trusted, so every package they signed still installs.
  Packages signed only with the new key (the first is the Docs plugin's
  listing) need 0.3.1 or later.
- wasmtime 49.0.4 (RUSTSEC-2026-0378, RUSTSEC-2026-0379).

## [0.3.0] - 2026-10-19

Documentation sites. Entries of a content type can now form a tree with
nested addresses, each post type can have its own page layout, search can be
scoped to a type or a term, and plugins gain what a content-syncing plugin
needs. The first such plugin, `vyasa-docs`, turns a folder of Markdown in a
GitHub repository into a documentation site with a sidebar, contents, pager,
versions and search.

### Added

- Hierarchical content types: entries can have a parent and an order
  (`parent_id`, `position`), nest to eight levels, and live at
  `/<base>/<path/to/entry>`. A tree view with drag and drop in the admin;
  `GET`/`PUT /api/v1/post-types/{type}/tree`; `parent`, `children` and
  `postTree` in GraphQL. Plugins declare `hierarchical` and a URL `base`;
  administrators tick "Hierarchical" on a content type.
- A layout tree per post type: `layout.json` may carry `types.<slug>`, and a
  plugin can supply a default tree for its own type.
- Search scopes: `type` and `term` on `GET /api/v1/search`, the GraphQL
  `search` field and `/search`.
- Plugin host: `fetch-with` (request headers, any status), managed entries
  (`put-entry`, `list-managed`, `delete-entry`; read-only in the editor until
  detached), `now-ms`, `call-budget-ms`, the capabilities `entries:sync` and
  `tasks:extended`, a one-minute budget for tasks that declare it, `secret`
  settings fields, extra forwarded request headers for plugin routes, fields
  declared by plugin post types, section `context`, child slots for plugin
  blocks, schema-driven block forms in the editor.
- `min_host_api` is now enforced: Vyasa 0.3 provides host API 2.
- The `vyasa-docs` plugin (`plugins/docs/`): documentation sites imported
  from a GitHub repository. See [docs/DOCS-SITES.md](docs/DOCS-SITES.md).
- Nested entries carry `BreadcrumbList` structured data.

### Changed

- Plugin HTML is sanitised with a wider allowlist (`id` prefixed `pl-`,
  `role`, `aria-*`, `data-*`, `nav`, `details`, `button` that never submits,
  a GET `form` to this site). Links may keep the `rel` values `prev`,
  `next`, `nofollow`, `ugc`, `sponsored` and `external`; `noopener
  noreferrer` is always added.
- The full-text index gains a field. On the first start of 0.3 the index is
  rebuilt automatically; search answers from the database for those seconds.
- Publishing an entry of a hierarchical type refreshes every cached page of
  that type, so sidebars stay current.

### Migrations

- `0053_hierarchy.sql` and `0054_managed_entries.sql`; forward-only. Back up
  before upgrading.

### Security

- ammonia 4.2.4 (RUSTSEC-2026-0373) and zip 9 (RUSTSEC-2026-0367, -0368,
  -0369). zip 8 remains only in the build script of `utoipa-swagger-ui`,
  which does not use the affected extraction code.

## [0.2.1] - 2026-10-15

### Added

- Every theme's footer now ends with a small "Made with Vyasa" link
  (`<p class="vy-credit">`). It is on by default and is switched off with
  the new `footer_credit` option, or under Settings > Advanced > "Show
  'Made with Vyasa' in the footer".

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

