# Changelog

All notable changes are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project
follows [semantic versioning](https://semver.org) as described in
[docs/VERSIONING.md](docs/VERSIONING.md).

## [Unreleased]

### Added
- `ai-eval` (developer tool, `crates/ai-eval`, not published): runs both agent harnesses on a Theme Studio brief set with an offline renderer, and records each run's cost, steps and proposal under spend caps. A dry run unless `--live`.
- The Sarathi harness can run a model without native tool calls, one JSON step at a time, and reports each call's prompt-cache write and read.
- The `ai_harness` option (`vyasa`, the default, or `sarathi`) picks the agent harness a site's AI surfaces run on, read per request, so both run side by side without a restart. On Sarathi the registry's primary model is called with native tool use; usage rows, budget caps, the step list and the proposal stay as on Vyasa's own loop. In Theme Studio a Sarathi run must state its criteria with a `plan` tool and render the page with `look` after its last edit before it may finish; a finish without that is sent back to the model, and after three refusals the run ends as out of steps.

### Fixed
- Claude models that refuse a forced tool call (`claude-sonnet-5-5`, `claude-opus-5-5`, the Fable and Mythos 5.1 and 5.2 models) work on Vyasa's own agent loop and assistants: they are asked to call the result tool instead of being made to, where every request used to be answered with a 400.
- OpenAI's reasoning models (`gpt-5` and the `o` series) can be used on Vyasa's own agent loop and assistants: requests to OpenAI send `max_completion_tokens`, not `max_tokens`, which those models refuse. Other OpenAI-compatible servers are sent `max_tokens` as before.
- Docs plugin 0.1.16: a fenced code block's language is the part of its info string before the first comma or space, lowercased: ```` ```rust,ignore ```` is Rust code. Vyasa 0.4.6 validates code languages, and refused every page with such a fence (`invalid code language "rust,ignore"`), which stopped the whole sync. A language the host would refuse anyway (for example `{.rust}`) is plain text.
- Docs plugin 0.1.16: a page the site refuses no longer stops the sync. It is imported as plain text (its Markdown in one code block), a generated section page without its overview, or, if the site refuses that too, left out with its child pages; the sync goes on and finishes. The status line on the settings page lists each such page with the reason (the first five, then a count), the public status counts them as `refused_pages`, and a page leaves the list once it imports as it is. Storage faults and quota pauses stop a run as before.
- Docs plugin 0.1.16: a page the site refused outright is not read or offered again by later runs of the same sync (a new commit, new settings or "Sync now" try it again), and the pages under a refused section are listed without being read, so a large import with refused pages still finishes. A page imported as plain text is listed before it is written, so a run paused by the write quota right after it still names it. The list is kept under the key-value store's 64 KiB (at most 100 pages; the status line says "or more" past that; a page in its plain-text form that did not fit is listed again once the list has room), and a run that pauses before it reaches the pages keeps the last list. Code fences with line-highlight syntax such as ```` ```js{1,3} ```` keep their language. A page imported before 0.1.16 keeps its code language until its source changes.

## [0.4.6] - 2026-11-27

### Changed
- The declared minimum supported Rust version is 1.97 (was 1.85, which no longer built: the plugin host's wasmtime needs 1.97); the pinned toolchain stays 1.99. CI checks that the workspace builds on 1.97.

### Fixed
- Hierarchical types: one entry whose parent is in the trash (or gone) no longer stops every write to its type. The tree check refused any change while such an orphan existed anywhere, so a docs sync that had trashed a group before moving its pages failed every run with `entry … has a parent (…) that is not in this type` and imported nothing. An orphan now counts as a root, as the tree view already showed it; a write that would create one (a move to a missing parent, a restore under a trashed parent) is still refused, and an entry whose parent and slug do not change keeps its stored path.
- Plugin quotas: writes, fetches, fetched bytes and model calls are counted per call budget too, like reads. A page render or hook that ran while the plugin's extended task was going got the task's larger limits and spent the same counters; it now keeps the ordinary limits on its own counters, and the task keeps its whole budget.
- Plugin quotas: an over-quota `fetch`, `fetch-with` or `ai-complete` returns `quota-exceeded`, as reads and writes do, instead of `capability-denied` (or `byte quota exceeded` for the fetched-bytes quota), so a task pauses instead of treating it as a missing grant. The audit row says which budget ran out. While an extended task runs, a plugin can make 20 model calls a minute on the ordinary budget and 20 on the task's (`task_ai_calls` lowers the task's share).
- Plugin quotas: fetched bytes keep the plugin's `VYASA_PLUGIN_QUOTAS` override when the plugin is enabled, disabled or reinstalled while a fetch is downloading. The override is matched by the plugin's name, which that change dropped from the cache, so the bytes were checked against the default limit.
- Docs plugin 0.1.15: a site with a main tree and three sections of about 680 documents each (2,672 in all) never imported one: the importer refused more than 2,000 documents in one sync before any write, every minute, with `pages: 0`. The cap is 5,000 documents now (over the main tree and every section together); each run is still limited by its request and write budgets.
- Docs plugin 0.1.15: a site made only of sections (Folder empty, no older versions) no longer lists the whole repository root as a main tree beside them, which imported every section's documents a second time.
- Docs plugin 0.1.15: an import that takes several runs no longer uses up GitHub's 60 calls an hour on a site without a token. Each run asked for every branch and tag head in full, and listed again every folder of more than about 600 documents (its listing, over 64 KiB, was refused by the key-value store). Now each head is fetched once and later runs ask with its etag (a 304 is free), and a listing keeps only its documents, split across values below 64 KiB, so later runs of the same commit list nothing and the import is not stopped part way by `GitHub rate limit or permission problem (403)`.
- A docs sidebar's `context.tree` holds up to 5,000 nodes, not 2,000, to match the docs plugin's 5,000-page import: on a site with several docs versions (about 2,700 pages) the sidebar left out the pages past the 2,000th.
- A plugin that errored or was disabled no longer costs a full `plugins` query per section, block or filter on every page: the host remembers the refusal for a minute (enable, disable and install clear it at once) and logs it once a minute, where it used to fail each render with nothing in the log.

## [0.4.5] - 2026-11-26

### Fixed
- Docs plugin 0.1.14: its script no longer reaches outside the docs page body. It added a Copy button (and `position: relative`) to every `<pre>` on the site and wrapped every table under `<main>` in a scrolling box with status pills, so a theme's own code panels on non-docs pages (a home page's `pre.code`) showed a second Copy control and the theme's room for it. The copy buttons and table wrapping now apply only inside the `docs-main` and `docs-content` sections, or inside an element a theme marks with `data-docs-copy` (to keep them on, say, blog posts), and the `.docs-copy` rule is scoped there too (with no added specificity, so theme overrides still apply).
- Docs plugin 0.1.14: a page whose name is one long token (`functions/tuple-sketch-summary-integer`) no longer makes a phone scroll sideways: the previous/next links shrink and break the name anywhere, and stack on narrow screens; the page title, breadcrumbs and sidebar links break long names the same way.
- Docs plugin 0.1.13: a sync the host throttles pauses and carries on, instead of starting over. A key-value read refused for the per-minute quota used to look like a missing key, so a busy minute made the importer forget its scan cache and its progress, fetch every page again and never finish (on a site of three docs versions it stayed at `pages: 0`). Refused reads and writes now stop the run where it is (`paused at the host's per-minute quota`), the task skips a minute whose own bookkeeping it cannot read, and the status keeps its page count.
- Plugin quotas: a call over its per-minute quota returns `quota-exceeded`, apart from `capability-denied` for a missing grant. An extended task (a scheduled sync) counts its reads apart from the plugin's page renders, against 3,000 a minute, so renders can no longer spend a sync's budget. A read refused for the quota writes one audit row a minute per plugin, not one per call. Every host read (`get-option`, `get-post`, `query-posts`, `kv-get`, `kv-list`) counts on its own call's budget. Plugins that matched `capability-denied` to detect throttling should match `quota-exceeded` instead.

### Added
- `VYASA_PLUGIN_QUOTAS` (`plugin_quotas`): the operator raises or lowers a plugin's per-minute limits by name, e.g. `vyasa-docs:task_reads=20000,reads=1200;*:writes=200` (`reads`, `fetches`, `bytes`, `writes`, `ai_calls`; `task_` for the extended-task budget; `*` for every plugin). A malformed value stops the server at start.

## [0.4.4] - 2026-11-24

### Fixed
- Docs plugin 0.1.12: the sidebar, pager and breadcrumbs no longer scan the whole page tree for every node (a quadratic walk). On a site with several docs versions (about 3,000 pages) a section render ran past the plugin's time limit and the repeated traps disabled the plugin. The tree is indexed once per render.
- A plugin's settings form shows what the plugin declares now, not what it declared when it loaded: the docs plugin's "Last sync" line was frozen at server start. `GET /api/v1/plugins/{id}/form` asks the plugin again; the admin uses it on open, after a save and every minute. Re-reading the form never counts against the plugin: a time or fuel trap there drops the cached instance, keeps the form from load and records no strike.

### Added
- Social cards: a theme can declare a `[social_card]` layout (background PNG, font, title and label boxes, post types), and with the new `social_cards` setting on (Settings → Advanced, shown when the active theme has a valid layout) an entry of those types with no image of its own gets a generated 1200×630 PNG as its `og:image`, with `og:image:width`, `og:image:height` and `og:image:alt`. Cards are served from `/_card/{id}-{key}.png`; the key changes with the title, label, theme version and the theme's card files, so a stale URL redirects to the current one. At most two cards render at once; rendered cards are cached in memory (`VYASA_SOCIAL_CARD_CACHE_BYTES`, 32 MiB by default) and purged at the edge when their entry changes. Drafts, password-protected entries and non-public post types get no card. `GET /api/v1/social-cards` reports whether the active theme can draw them, and names any post type in the layout the site does not have (a notice: cards stay on); activating such a theme names them too. The starter docs theme has a layout for docs pages.
- The plugin detail view shows the plugin's status and its scheduled tasks (interval, last run, last error), and says that a turned-off plugin's settings come back when it is on.
- Archive templates can draw blog cards: each entry in `posts` carries `image` (the featured media's URL, width, height and alt, or none), `sticky`, `iso_date` and its `terms`; `featured` lists the pinned entries and `categories` lists the categories with an entry count and archive URL.
- Entry pages of flat post types get `newer`, `older` (the neighbouring published entries of the same type and language) and `related` (up to three entries sharing the most terms, newest first). Publishing or changing an entry drops the cached pages of its neighbours and related entries only, not every page.

## [0.4.3] - 2026-11-22

### Fixed
- Search finds text in tables, code blocks and quotes, not only paragraphs and headings: a config key in a reference table or an error class in a code block now matches (docs QA-W-4). The AI assist text is unchanged. An existing index keeps its old bodies until it is rebuilt: run `vyasa search reindex` (or `POST /api/v1/search/reindex`, or Site health → Rebuild index) after upgrading.
- A plugin refresh keeps the plugin's scheduled tasks declared until the new declarations replace them, and a task that was claimed but has no declaration is skipped instead of run on the ordinary budget.
- A plugin call that runs past its time or fuel limit (an epoch-deadline or fuel trap) no longer disables the plugin for good on the first occurrence: it is marked degraded and stays enabled, and the third such trap within ten minutes disables it as before. A busy server (a deploy publishing pages while a sync runs) could trap a healthy plugin once and take its pages down until an administrator re-enabled it. Any trap now records its reason on the Plugins page; before, a disabled plugin showed no cause. A settings save (or a restart) that a plugin refuses now leaves it degraded and serving on its previous settings, not errored: an errored plugin refuses every later call, so the next settings save could not heal it, and the docs import of a staging deploy left vyasa-docs off until an administrator re-enabled it.

- A page (About, Subscribe, a docs page) is no longer described as an article: it gets `og:type website`, a `WebPage` JSON-LD block and no `article:*` dates.
- `article:modified_time` and `dateModified` are never earlier than the publication time (a scheduled post is saved before it goes out).
- Each menu `<nav>` has an accessible name: the block's `label` setting, else the menu's slug.

### Added
- `GET /llms-full.txt`: every readable entry's Markdown in one file (the same set as `llms.txt`, each body inline), so a model reads the whole site in one fetch. `llms-full` is now a reserved type slug.
- A theme's `font_faces` entries take optional `weight`, `style` and `display`, printed as `font-weight`, `font-style` and `font-display` in the `@font-face` rule. Two files of one family (400 and 700, roman and italic) are no longer reported as a duplicate.

### Added

- A theme may bundle `assets/manifest/site.webmanifest`; it is served as `application/manifest+json` under `/theme-assets/manifest/` and linked from every page head.

### Fixed

- A page description taken from an excerpt or `seo_description` never carries markup (a hand-written excerpt with inline HTML left a half tag in `<meta name="description">`).
- Newest and Oldest order by publication time, so the `latest-posts` block lists the most recently published entry first; a draft sorts by its creation time.
- `put-entry`: an entry whose `source-hash` is unchanged but whose stored status is `draft` while `published` is asked (or the reverse) now has its status repaired (`updated`) instead of answering `unchanged`; `scheduled`, `private` and `trash` entries are left alone. A root left in draft by an earlier sync kept the docs tree empty.

### Security
- Dependencies: lettre 0.11.24 (SMTP STARTTLS, dot-stuffing and header-handling advisories) and wasmtime / wasmtime-wasi 50.0.3 (plugin host advisories).

## [0.4.2] - 2026-11-13

### Fixed
- An upgrade clears a plugin's errored status, and a per-minute quota refusal while applying its settings leaves the plugin degraded instead of errored (#115).
- A docs entry has one canonical address: a trailing slash or a dotted version segment (`/docs/app/0.2/`) answers with a 301 to the stored address (#117).
- `ammonia` 4.2.5 and `quinn-udp` 0.5.16, for RUSTSEC-2026-0533 and RUSTSEC-2026-0538 (#118).
- Docs plugin 0.1.6: re-applying unchanged settings (every site deploy) no longer writes the settings key, and a refused write with settings already stored no longer fails `init`. A deploy right after a sync used to hit the per-minute write quota and leave the plugin errored.
- Docs plugin 0.1.5: a site of over 600 pages synced never finished, because the importer read one key-value entry per page and the host allows 600 reads a minute even in an extended task. The scan cache is now a few packed entries (about ten reads a run) and is saved however a run ends, so a cut-short run resumes.

### Added
- The contact path of `POST /form` keeps every other posted field (`company`, `role`, a `wants_contact` checkbox ...) in the submission, at most 20 fields of 1000 characters, and Admin submissions lists them; `GET /api/v1/audience/submissions` returns them as `data` (#119).
- Docs plugin 0.1.4: a section slug may be `product/version` (`app/2.0`), served at `/docs/app/2.0/` with its own sidebar and search scope; a hidden `product` root is generated when no section of that name exists.

## [0.4.1] - 2026-11-10

### Fixed

- The release workflow builds the macOS arm64 binary on `macos-15`; GitHub
  retired the `macos-14` runner image, so the 0.4.0 binaries and GitHub
  release were never published. 0.4.1 has the same code as 0.4.0.

## [0.4.0] - 2026-11-10

### Added

- A theme can place the "Made with Vyasa" credit itself: `"credit": "theme"` in
  `layout.json` plus `{{ vyasa.credit_html | safe }}` in its footer template.
  The engine then skips its own line (no duplicate) but still adds it if the
  rendered page has none. See docs/THEMES.md.
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
- vyasa-docs: a version can be read from another branch or tag
  (`next = docs @ main`), so the unreleased docs can be served at
  `/docs/next/` beside the latest release. The webhook accepts pushes to
  those refs.
- `discourage_indexing` site option (Settings > Advanced) for staging sites:
  `robots.txt` answers `Disallow: /` and every response carries
  `X-Robots-Tag: noindex, nofollow`. Off by default.
- SVG uploads (logos, icons) in the media library. Each file is rewritten through an allow-list sanitiser before it is stored: scripts, `foreignObject`, styles, images, animation, event handlers and every reference that leaves the file are removed, and a file with a DOCTYPE or a root other than `<svg>` is refused. `.svgz` stays refused. SVGs are served with `Content-Security-Policy: default-src 'none'; sandbox` and `nosniff`.
- `color_scheme` site option (`system`, `light`, `dark`; Settings > Advanced): the scheme first-time visitors see. `light`/`dark` set `data-theme` on `<html>`; a visitor's own toggle still wins, and `system` (default) follows the device.
- Built-in colour-scheme toggle: every page links `/theme-assets/theme-toggle.js` in `<head>`. It applies the visitor's stored choice before first paint (no flash), and any element with `data-vy-theme-toggle` switches light/dark and remembers it (`localStorage` key `vy-theme`). Works under `script-src 'self'`; themes only add the button.

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

