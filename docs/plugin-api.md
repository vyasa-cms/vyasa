# Vyasa Plugin API — contract reference (`vyasa:plugin@1.0.0`)

The authoritative interface lives in
[`crates/plugins/wit/host.wit`](../crates/plugins/wit/host.wit). This document explains every item, its capability requirements, and the limits the host enforces. For packaging + signing see [`plugin-sdk/sign.sh`](../plugin-sdk/sign.sh) and the lifecycle REST (`POST /api/v1/plugins`).

## Two worlds

```wit
package vyasa:plugin@1.0.0;

interface host { … }              // imported by plugins (host-provided)

world vyasa-plugin {              // the base world
    use host.{filter-stage, event-kind};
    export init / filter / action / register-blocks / register-routes;
}

world vyasa-plugin-v2 {           // a strict superset
    // …everything above, plus:
    export handle-request / render-block / run-task /
           register-schedule / register-admin / register-assets /
           register-post-types / register-taxonomies /
           filter-at / on-event;
}
```

A plugin implements exactly one world, and the host works out which when
it loads the component: it resolves the superset world's export indices
once, and a component that does not have them is served by the base path
forever after. Every v2 entry point answers `Unsupported` for such a
plugin — an ordinary answer, not a failure, so nothing degrades a plugin
for having been built before these existed.

That is why the new exports are a superset world rather than
`@2.0.0`: bumping the package version would have invalidated every plugin
binary already installed.

**Host imports only ever grow.** A component imports just the functions it
calls, and a host offering more than a guest asks for still satisfies it.
So storage, post writes and comment reads were added to
`vyasa:plugin@1.0.0` in place rather than to a new interface version.

## Host functions (imported)

| Function | Signature | Capability | Notes |
|---|---|---|---|
| `log` | `(log-level, string)` | none | Structured; level maps to tracing. |
| `get-post` | `(u64) -> result<post-summary, string>` | `db:read:posts` | Broker-gated; summary only. |
| `query-posts` | `(option<string>, u32) -> result<list<post-summary>, string>` | `db:read:posts` | Limit clamped to 100. |
| `get-option` | `(string) -> result<option<string>, string>` | `db:read:options` | Public keys only. |
| `emit-event` | `(event-kind, string) -> result<_, string>` | `event:emit` | Payload must be `{"id": <number>}`. `comment-added` and `request-render` cannot be emitted by a plugin. |
| `fetch` | `(string) -> result<string, string>` | `net:fetch:<host>` | HTTPS GET only. 5 s timeout, 1 MiB cap, redirects refused (a redirect would spend a grant on another host). |
| `ai-complete` | `(system: string, user: string) -> result<string, string>` | `ai:text` | A plain-text reply from the site's default text model (Models page). Budgeted (20 calls/minute per plugin) and logged under `plugin:<name>`; prompts over 20k characters and system prompts over 8k are refused. Refused inside the `page-html` filter; inside every other render-path hook (`filter`, `filter-at`, `render-block`) a reply that takes over 1.5 s is abandoned with `ai-complete timed out`. |
| `kv-get` / `kv-list` | `(string) -> result<option<string>, string>` / `(string, u32) -> result<list<string>, string>` | `kv:read` | The plugin's own namespace. Two plugins using the same key never see each other's value. |
| `kv-set` / `kv-delete` | `(string, string) -> result<_, string>` / `(string) -> result<_, string>` | `kv:write` | Keys ≤ 256 B, values ≤ 64 KiB, 10 000 keys per plugin. Write implies read. |
| `list-comments` | `(u64, u32) -> result<list<comment-summary>, string>` | `db:read:comments` | Approved comments only, newest first, no email addresses. Capped at 200. |
| `create-post` | `(new-post) -> result<u64, string>` | `db:write:posts` (+ `db:publish:posts` to publish) | `post-type` may be `post`, `page`, or any registered custom type — a forum plugin creating a `topic` is the point; `block` is refused. Body is HTML, stored as a single sanitized `html` block. Slug derived and de-duplicated. Attributed to the oldest administrator; ownership recorded in `plugin_posts`. |
| `update-post` | `(u64, post-patch) -> result<_, string>` | `db:write:posts` (+ `db:publish:posts` to publish) | **Only rows this plugin created.** Anything else returns `not your post`. |
| `current-viewer` | `() -> result<option<viewer>, string>` | `viewer:read` | Id, display name and role (a built-in name, or a custom role's slug) of the signed-in visitor — never an email or a token. **Populated only inside `handle-request`**; `none` everywhere else. |
| `get-post-meta` / `set-post-meta` | `(u64, string) -> result<option<string>, string>` / `(u64, string, string) -> result<_, string>` | `db:read:posts` / `db:write:meta` | Stored at `meta.plugin.<plugin>.<key>` on the post, so it travels with the content. Namespaced, so two plugins using `rating` never collide. Keys ≤ 64 B, values ≤ 16 KiB. |
| `send-mail` | `(string, string, string) -> result<_, string>` | `mail:send` | Plain text, queued like every other message. `to` must be a registered user's address or the literal `site-admin`, which the host resolves without telling the plugin the address. |

### Capabilities

Declared per-plugin in its `manifest.toml` and enforced by the capability
broker. The names the parser accepts are exactly:

| Capability | Grants |
|---|---|
| `db:read:posts` | `get-post`, `query-posts` |
| `db:read:options` | `get-option` (public keys only) |
| `db:read:comments` | `list-comments` |
| `db:write:posts` | `create-post`, `update-post` — drafts, and only rows this plugin created |
| `db:publish:posts` | on top of the above, may set `published` |
| `kv:read` / `kv:write` | the plugin's own key/value namespace (write implies read) |
| `db:write:meta` | `set-post-meta`, inside the plugin's own namespace on any post |
| `viewer:read` | `current-viewer` |
| `mail:send` | `send-mail` |
| `ai:text` | `ai-complete` |
| `net:fetch:<host>` | `fetch`, subject to fetch and byte quotas |
| `log:write` | `log` |
| `event:emit` | `emit-event` |
| `html:page` | the `filter` export is called at the `page-html` stage |
| `assets:script` | the `js` half of `register-assets` is served and linked |

`db:write:posts` and `db:publish:posts` are separate on purpose: the
difference between a draft waiting for review and text live on the site is
the whole reason an operator reads the capability list before installing
anything. Neither is a licence to edit content a person wrote — ownership
is checked against `plugin_posts` on every write.

An unknown capability string is rejected at install. A call without its
capability returns `Err("capability-denied")` — it does not trap — and the
denial is written to `plugin_audit`. Quotas (600 DB reads/minute and 20
`ai-complete` calls/minute by default) are audited the same way.

`html:page` and `assets:script` gate what reaches visitors rather than a
host call. A plugin without `html:page` is simply not called at the
`page-html` stage (it still receives `post-trashed`); a plugin without
`assets:script` has its declared JS dropped at boot with a warning, while
its CSS is served as before. **Behaviour change:** plugins installed
before these capabilities existed lose both powers until they are
reinstalled from a package that declares them.

### Limits

| Resource | Limit |
|---|---|
| Memory | 64 MiB default (per-instance) |
| Fuel / CPU | 1 billion fuel per call (~0.5 s); overrunning traps the call |
| Wall clock | 40 epoch ticks x 50 ms = 2 s per call |
| Concurrency | one instance and store **per call**: plugin calls run in parallel and share no state between requests |
| Package size | 10 MiB per `.vyplugin` |
| `ai-complete` prompt | 20 000 characters user, 8 000 system; 20 calls per minute per plugin |
| Writes | 120 per minute per plugin (kv and post writes together) |
| kv | 256 B keys, 64 KiB values, 10 000 keys per plugin |
| Plugin blocks per page | 32; each is a fresh instantiation |
| Route request body | 256 KiB in, 1 MiB out |
| Declared assets | 256 KiB CSS, 512 KiB JS |
| Scheduled tasks | interval clamped to 60 s … 24 h, 16 per plugin |
| Post meta | 64 B keys, 16 KiB values, 64 keys per plugin per post |
| Mail | 200-character subject, 32 KiB body, charged to the write quota |
| Named filter payloads | 64 KiB in and out; a larger reply is ignored |

Individual strings crossing the boundary are otherwise uncapped. A trap
(including a guest panic) fails that one call, marks the plugin `errored`
and disables it; it never fails the request that triggered it.

## Plugin exports

### `init(config-json: string) -> result<plugin-info, string>`

Called once on load with the plugin's stored settings as a JSON object
(`plugin_settings` rows, key to value). Return `{ name, version }`; an
`err` aborts activation.

### `filter(stage, content: string) -> string`

Stages: `content`, `page-html`, `feed-item`, `comment-body`. Return value replaces the content verbatim (already escaped/sanitized as appropriate for the stage). `page-html` is only called for plugins granted `html:page`.

### `action(kind: event-kind, payload-json) -> result<_, string>`

v1 events: `post-published`, `post-updated`, `post-trashed`, `comment-added`, `request-render`. Actions are fire-and-forget; errors are logged, never surfaced to visitors.

### `register-blocks() -> string`

JSON array of editor block descriptors:
```json
[{ "kind": "acme/price", "title": "Price", "icon": "€" }]
```

`kind` must be `namespace/name` (lowercase letters, digits, hyphens). The
slash is what stops a plugin from ever shadowing a core kind. A kind
another plugin already declared is refused — first declaration wins, so
installing a second plugin cannot take over the rendering of blocks
already sitting in published documents.

Declared kinds appear in the editor's insert menu under **Plugins**, and
their attributes are edited as JSON (the plugin owns the schema; the admin
does not pretend to know it).

### `register-routes() -> string`

JSON array: `[{ "method": "GET", "path": "widget" }]`. Routes are mounted
at `/api/v1/plugin/<plugin name>/<path>` and served by `handle-request`. A
path may end in `*` to match the rest of the path.

### `render-block(kind, attrs-json) -> result<string, string>` *(v2)*

Called once per block instance, before the theme renders the document. The
returned HTML is sanitized by the host (a widened allowlist that permits
`class`, so a plugin can style what it emits), then wrapped in
`<div class="vy-plugin-block" data-block="…">`.

Escaping values that came from the document is still the plugin's job: the
host's sanitizer removes script, it does not fix broken markup.

A block whose plugin is disabled, uninstalled or trapped renders as
`<!-- vyasa: unresolved plugin block … -->`. The stored document is
untouched, so re-enabling the plugin brings the block back.

### `handle-request(http-request) -> result<http-response, string>` *(v2)*

Serves one request to a declared route. The boundary is narrowed in both
directions:

* the guest sees `accept`, `accept-language`, `content-type`,
  `user-agent` and `x-requested-with` — **never** `cookie` or
  `authorization`;
* the reply may set only `content-type`, `cache-control` and `location`;
* `content-type` must be one of text/plain, text/html, text/css,
  application/json, application/xml, text/xml, image/svg+xml — anything
  else falls back to text/plain;
* `location` must be same-site (one leading slash), because a plugin
  redirecting a visitor to another origin is a phishing primitive;
* the status must be in 100–599 — the type accepts anything up to 999, so
  a plugin answering `999` would otherwise reach a browser;
* `x-content-type-options: nosniff` is always added.

### Why the viewer is only available here

A page render is cached, and the cached HTML is served to every visitor.
A filter or a block that personalised on `current-viewer` would show one
person's page to the next, so the host does not populate it there. A
plugin route is rendered per request and cached by nobody, which is the
one place it can be honest.

### `run-task(name) -> result<_, string>` + `register-schedule() -> string` *(v2)*

`[{ "name": "sync", "every-seconds": 3600 }]`. Intervals below 60 s are
raised to 60 rather than rejected — asking to run every second is a
reasonable intention at the wrong granularity, and never running is a
worse answer.

State lives in the `plugin_tasks` table, so a task scheduled every six
hours is not re-run by every deploy. The runner claims a due task with an
`UPDATE … RETURNING` before running it, so two servers against one
database never both run it, and a task that hangs is not retried on the
next tick forever.

### `register-admin() -> string` *(v2)*

```json
{ "title": "Bookshelf", "description": "…",
  "fields": [{ "key": "star", "label": "Star glyph", "type": "select",
               "options": ["★", "●"], "default": "★", "help": "…" }] }
```

Types: `text`, `textarea`, `number`, `boolean`, `select`. The admin renders
these generically; what an operator saves is what `init` receives on the
next boot. A plugin declares *data*, never markup — that is the whole
difference between extending the admin and running third-party code in it.

### `register-assets() -> string` *(v2)*

`{ "css": "…", "js": "…" }`. Served from `/plugin-assets/<content
hash>.css|js` with a one-year immutable cache, and linked from every
public page's `<head>` — the `js` half only for a plugin granted
`assets:script`. External files rather than inline tags: it keeps
rendered pages cacheable and gives a content-security policy something to
name.

A plugin's JS runs with the page's full privileges. That is not a new
trust boundary — installing the plugin already required a signature from a
key the operator trusts — but it is worth saying out loud.

### `register-post-types() -> string` *(v2)*

```json
[{ "slug": "book", "singular": "Book", "plural": "Books",
   "public": true, "has-archive": true }]
```

Entries live at `/<slug>/<entry-slug>` with an archive at `/<slug>`. A
page with that slug still wins, so a plugin cannot hide one by picking its
name. Reserved slugs (`post`, `page`, `block`, `api`, `admin`, `feed`,
`category`, `tag`, `search`, …) are refused, as are names outside
`^[a-z][a-z0-9-]{1,31}$` — the `posts.type` column enforces the same
grammar.

Rows survive their plugin: uninstalling one leaves its entries stored and
readable (they simply stop being served), rather than making the post list
fail to decode.

**Slugs shared with the admin's content types (phase 99).** Administrators
create content types in the same registry, so a slug has one owner:

- **The admin type wins.** At boot, admin types are registered before any
  plugin declares its types. A plugin declaring an admin type's slug (as a
  post type *or* a taxonomy) loses that declaration: it is skipped with an
  error in the log and the plugin shows as degraded, with the reason, in
  plugin health and on the Plugins page.
- **Enable and rollback are refused** (`409`, naming the slug) when any
  declared post type or taxonomy clashes with an admin type. Registration
  is all-or-nothing: a refused enable leaves none of the plugin's types or
  taxonomies registered, and the plugin stays off with the reason recorded.
- **Settings save, install and upgrade of an enabled plugin degrade
  instead of refusing**: the clashing type is skipped and the plugin keeps
  running in a degraded state, so a settings save never silently turns a
  plugin off.
- An administrator cannot create a content type whose slug an enabled
  plugin declares (or that is a live taxonomy's slug).
- Plugins are refused every core reserved slug, including every first path
  segment the public router serves — the same list admin content types are
  refused.
- **Disabling a plugin unregisters its post types at once**: `?type=<slug>`
  on the REST listing then answers `400` (the type no longer parses), and
  the entries stay stored. A slug another enabled plugin still declares
  stays registered.
- **Plugin taxonomies are never unregistered until restart** (existing
  behaviour): once a plugin's taxonomy has been live in a process, an admin
  type with that slug is refused until the server restarts, even after the
  plugin is disabled.

### `register-taxonomies() -> string` *(v2)*

```json
[{ "slug": "shelf", "singular": "Shelf", "plural": "Shelves",
   "hierarchical": false, "public": true }]
```

Terms archive at `/<slug>/<term>`, the same namespace custom post types
use — so a taxonomy may not take a slug a post type already holds, and
vice versa. The same reserved list and name grammar apply, enforced again
by the `terms.taxonomy` column.

### `filter-at(point, payload) -> string` *(v2)*

The string-keyed filter. **A point the plugin does not handle must return
`payload` unchanged** — that is what lets the host run every plugin
through every point without any of them knowing about the others.

| Point | Payload | Fires |
|---|---|---|
| `seo-title` | the title text | every public page's `<head>` |
| `seo-description` | the description text | every public page's `<head>` |
| `post-title` | the title text | listings and entry pages |
| `excerpt` | the excerpt text | listings and `<head>` |
| `search-query` | the visitor's terms | before the index is queried, so a rewrite changes what is searched |

New points are additive: a constant in `crates/api/src/plugin_hooks.rs`
and a call where it belongs. Nothing recompiles on the guest side and no
component is invalidated, which is the entire reason these are strings
rather than another WIT enum.

### `on-event(name, payload-json) -> result<_, string>` *(v2)*

The string-keyed event. Fire-and-forget: a plugin reacting to one cannot
slow down, or fail, the thing that happened.

| Event | Payload |
|---|---|
| `login-succeeded` | `{"user_id", "role", "custom_role"}` |
| `login-failed` | `{}` — never the address tried, nor why (an unconfirmed account is refused like a wrong password) |
| `user-created` | `{"user_id", "role", "custom_role"}` |
| `user-registered` | `{"user_id", "role", "custom_role"}` — a visitor registered; the account is not confirmed yet, and is deleted after a week if it never is (no `user-created` follows) |
| `user-role-changed` | `{"user_id", "role", "custom_role"}` |
| `user-deleted` | `{"user_id"}` |
| `media-uploaded` | `{"media_id", "mime", "bytes"}` |
| `post-saved` | `{"post_id", "status", "post_type"}` — every save, unlike `post-published` |
| `comment-approved` | `{"comment_id", "post_id"}` — when it becomes visible, unlike `comment-added` |
| `term-created` | `{"term_id", "taxonomy", "slug"}` |
| `theme-activated` | `{"theme_id", "name"}` |
| `option-changed` | `{"key"}` — the key only; an option may hold a secret |

In the four account events `role` is always the built-in role, which is
`subscriber` for an account holding a custom role; `custom_role` is then
that role's slug, and `null` otherwise. `current-viewer`'s `role` is the
role the account really holds: the custom role's slug when there is one.

The four `event-kind` cases (`post-published`, `post-updated`,
`post-trashed`, `comment-added`) still arrive through `action`, for
base-world plugins.

### Designer sections (`register-sections` / `render-section` points)

Sections — the things the theme studio composes pages from — are a plugin
surface too, and they arrive through `filter-at` rather than new exports,
exactly as that export's own doc says new surfaces should: a point a
plugin does not handle returns the payload unchanged, so every installed
binary keeps working.

**`filter-at("register-sections", "")`** — asked once when the plugin's
surface is collected. Reply with a JSON array (returning the payload
unchanged means "none"):

```json
[{ "kind": "store/product-grid",
   "title": "Product grid",
   "category": "commerce",
   "description": "Bound products as cards",
   "settings-schema": { "type": "object", "properties": { } },
   "sample": { "bind": { "source": "product", "sort": "newest", "limit": 4 } },
   "binds": true,
   "inline": ["heading"] }]
```

`kind` must be `namespace/name` — the slash is what stops a plugin from
shadowing `hero`, and first declaration wins across plugins, the same two
rules blocks follow. `category` is one of `structure`, `marketing`,
`content`, `commerce`, `community`, `navigation`. The descriptor's
description, schema, sample and inline keys feed the insert library, the
inspector's form and both AI assistants directly: registering a section
puts it in all of them, with no host code naming your domain.

**`filter-at("render-section", payload)`** — once per instance at render
time. The payload is `{"kind", "settings", "bound", "editor"}`. When the
section declared `"binds": true` and its settings carry a `bind` object,
**the host resolves the binding** with the same query path and visibility
rules core sections use, and `bound` holds the entries
(`{id, title, url, excerpt, date, thumb_url}`) — a section renderer needs
no database capability. Return HTML for a kind you own; return the
payload unchanged for one you do not. The reply passes the same widened
sanitizer as `render-block` and is wrapped
`<div class="vy-plugin-section" data-plugin="…">` inside the ordinary
section wrapper, so scopes, `hide_on` and the theme's `--vy-*` custom
properties cascade into your markup — style against those tokens, not
hexes, or dark bands will disagree with you.

**Lifecycle.** While your plugin is disabled the section validates as a
warning, renders as `<!-- vyasa: unresolved plugin section … -->`, and
the stored layout is untouched; re-enabling brings it back. Output caches
with the page like every `filter` result: anything per-viewer (a cart
badge) is client-side JavaScript in your markup calling your registered
routes, which is where `current-viewer` lives.

## What actually fires today

| Extension point | State |
|---|---|
| `filter` at `page-html` | works — every rendered page, before the render cache; needs `html:page` |
| `filter` at `content` | works — the entry body before the template wraps it |
| `filter` at `content` (auth challenge) | works — a `{"challenge_required":true}` reply makes login return 401 |
| `filter` at `comment-body` | works — a visitor's comment before it is stored |
| `filter` at `feed-item` | works — the rendered feed before delivery |
| `action` for all four event kinds | works |
| `register-blocks` + `render-block` | works — insertable in the editor, stored in the document, rendered on the page |
| `register-routes` + `handle-request` | works — the plugin serves the response |
| `register-schedule` + `run-task` | works — recorded in `plugin_tasks`, run by the scheduler |
| `register-admin` | works — rendered as a settings form, fed back to `init` |
| `register-assets` | works — served content-addressed, linked from every page; JS needs `assets:script` |
| `register-post-types` | works — storage, public URLs and archives |
| `register-taxonomies` | works — storage, term archives |
| `filter-at` | works — five named points, plus `register-sections` / `render-section` |
| `on-event` | works — eleven named events |
| `current-viewer`, post meta, `send-mail` | work |
| `filter` at `seo-title` | not dispatched: the WIT has no stage for it, so a guest cannot tell it apart from `content` |

Because filtered HTML is what gets cached, a page already in the render
cache will not re-run your filter — restart or change the content to see
an edit.

## Reference plugins

`plugin-sdk/examples/bookshelf/` uses every extension point above — a
custom post type, a custom block, a public route, a scheduled task, a
settings form and a stylesheet — in about 200 lines. It is the fixture
behind `crates/plugins/tests/v2_contract_tests.rs` and
`crates/api/tests/plugin_integration.rs`, so it is compiled to
WebAssembly and exercised on every run.

`plugin-sdk/examples/footer-note/` is the smallest useful example: one
filter and one host call, on the base world.

`plugin-sdk/examples/storefront/` and `plugin-sdk/examples/forum-lite/`
are the phase-55 reference plugins: a commerce plugin (products, designer
sections with prices, a signed-in cart, order requests by mail) and a
community plugin (topics as entries, replies as the site's own comments).
They are the worked answers to "how do I build WooCommerce/bbPress on
this" and are exercised end-to-end in
`crates/api/tests/reference_plugins.rs`.

`examples/plugins/{two-factor,multilingual,spam-guard}` are **host-side
Rust libraries, not plugins** — they have no `wit_bindgen`, no `cdylib`,
and no manifest. They read as design sketches for the hooks above.

## Building, signing and installing

```bash
rustup target add wasm32-wasip2
cp -r plugin-sdk/template plugin-sdk/examples/my-plugin
# fix the WIT path in src/lib.rs if you move the crate
cd plugin-sdk/examples/my-plugin
cargo build --release --target wasm32-wasip2
wasm-tools validate target/wasm32-wasip2/release/*.wasm
```

A package is a zip of `manifest.toml`, `plugin.wasm` and `signature.txt`.
A hand-uploaded package is accepted only when the signature verifies
against a key in `package_trusted_keys` (`plugin_trusted_keys` still
works as an alias); a marketplace package is checked against the
`author_key` its listing names. Mint a key and sign with the CLI:

```bash
vyasa plugin keygen                 # prints a secret and a public key
export VYASA_PACKAGE_TRUSTED_KEYS=<public hex>  # then restart the server

mkdir pkg && cp target/wasm32-wasip2/release/my_plugin.wasm pkg/plugin.wasm
cat > pkg/manifest.toml <<'TOML'
name = "my-plugin"
version = "0.1.0"
min_host_api = 1
capabilities = ["db:read:posts"]
TOML
VYASA_SIGNING_KEY=<secret hex> vyasa plugin pack pkg --out my-plugin.vyplugin
```

Install through Admin → Plugins, over HTTP:

```bash
curl -b "$SESSION" -F file=@my-plugin.vyplugin -F trust_confirm=true \
     http://localhost:3000/api/v1/plugins
```

or headlessly on the server, which needs no browser session:

```bash
vyasa plugin install my-plugin.vyplugin --enable
```

All three run the same checks: size, zip safety, capability names, and
the signature against `package_trusted_keys`. To publish the package for
every Vyasa install, see [PUBLISHING.md](PUBLISHING.md).

Installing does not enable it; enable it separately.

**Through the admin or the REST API, no restart is needed.** Installing,
enabling, disabling, rolling back, uninstalling and saving a setting each
withdraw everything the plugin contributed and — if it is still enabled —
collect it again: `init` runs with the current settings, and its hooks,
blocks, routes, assets, post types, taxonomies and scheduled tasks are
re-registered against the running process.

**The CLI is the exception.** `vyasa plugin install` writes to the
database from a *separate process*, so the running server does not learn
about it until it restarts. Use it for scripted deploys that restart
anyway; use the admin for a live change.
