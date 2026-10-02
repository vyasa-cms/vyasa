# Vyasa Architecture

This is the map for contributors: how the pieces fit, and the rules that keep
them fitting. [CONTRIBUTING.md](../CONTRIBUTING.md) lists the same rules as a
checklist.

**Core decisions:** a fresh WordPress-inspired CMS (no WordPress
compatibility) · single site per install · PostgreSQL · REST **and** GraphQL ·
AI through cloud providers (Anthropic and OpenAI-compatible APIs, with
failover and budgets) · themes are data, not code · WebAssembly plugins with
declared capabilities · one server binary plus Postgres.

## Why (bottlenecks of WordPress this fixes)

| WordPress problem | Vyasa answer |
|---|---|
| Plugins run arbitrary PHP with full DB/FS access | WASM plugins in wasmtime, capability-based, fuel+memory limited |
| Themes are executable PHP (theme editor = RCE) | Themes = tokens + layout + sandboxed Tera; no admin file editor |
| `wp-cron` is lazy and unreliable | Real scheduler + Postgres-backed queue (`jobs` crate) |
| Content is HTML soup with block comments | Content is structured JSONB blocks; rendered HTML is a cache |
| `wp_postmeta` unbounded EAV sprawl | Typed, schema-validated JSONB meta |
| REST only (GraphQL via plugin) | REST (utoipa/OpenAPI) + async-GraphQL, same service layer |
| Revisions grow forever | Adaptive revision pruning |
| Search is `LIKE` queries | Tantivy BM25 index |
| Deployment = PHP + web server + fpm + files | One server binary plus the built admin files, Postgres only |

## System overview

```
                    ┌────────────────────────────────────────────┐
  Browser ─────────►│              vyasa binary             │
  Headless clients  │  ┌────────┐ ┌─────────┐ ┌──────────────┐  │
  (Next/Astro) ────►│  │  REST  │ │ GraphQL │ │ Public site  │  │
                    │  │ utoipa │ │ async-  │ │ Tera themes  │  │
                    │  └───┬────┘ └────┬────┘ └──────┬───────┘  │
                    │      └─────┬─────┴─────────────┘          │
                    │        ┌───▼────┐  ┌───────────┐          │
                    │        │ core   │  │ plugins   │◄─ WASM   │
                    │        │ domain │◄─│ wasmtime  │  .wasm   │
                    │        └───┬────┘  └───────────┘          │
                    │        ┌───▼────┐  ┌───────────┐          │
                    │        │  db    │  │ themes/ai │          │
                    │        │ repos  │  │ search/jobs│          │
                    │        └───┬────┘  └───────────┘          │
                    └────────────┼─────────────────────────────┘
                                 ▼
                            PostgreSQL
```

## Crates & dependency direction (strict, one-way)

```
common <- db <- core <- {themes, plugins, ai, search, jobs} <- api
```

- `common` — error taxonomy (thiserror), config, tracing, IDs/slugs.
- `db` — sqlx PgPool, embedded migrations, repositories. Data access only.
- `core` — posts, blocks, taxonomy, media rules, users/RBAC, comments,
  domain events. Pure; no HTTP types.
- `themes` — design tokens, layout composition, sandboxed Tera, renderer
  (ammonia), `.vytheme` packages.
- `plugins` — wasmtime host, WIT contracts, capability broker, hook registry,
  lifecycle.
- `ai` — LLM provider trait, structured output, theme generation, editor
  assist, cost tracking.
- `search` — Tantivy index + query pipeline.
- `jobs` — scheduler + queue workers.
- `api` — **binary** `vyasa`: Axum REST + GraphQL + public rendering +
  admin SPA serving. The only composition root.
- `testkit` — test-only: a private Postgres database per test, cloned from a
  migrated template, and the server under test.

Rules that keep the layering honest: `core` never imports HTTP or GraphQL
types; SQL lives only in `db`; only `api` depends on `axum` and
`async-graphql`.

## Request path and authorization

Every request passes the same middleware stack (in `crates/api/src/main.rs`):
client-IP resolution (trusting only configured proxies), request context and
metrics, timeouts and body limits, CSRF checks for cookie sessions, rate
limits, and principal resolution (session cookie or API key).

Authorization has two halves:

- **Routes declare who may call them.** Every `/api/v1` route is registered
  through `authz::Guarded` with an `Access` — `Public`, `Authenticated`,
  `Cap(capability)` or `AnyOf(capabilities)`. The full table is generated
  into [ROUTE-ACCESS.md](ROUTE-ACCESS.md), and a route × role matrix test
  proves each guard refuses whom it should.
- **Policy decides about specific things.** Whether this user may edit that
  post, see a draft, read entries of a non-public content type, or assign a
  role is decided only by functions in `crates/api/src/policy.rs`, never
  inline in a handler. REST, GraphQL and the public site all ask the same
  functions, so the answers cannot drift apart.

Capabilities come from roles: five built-in roles plus administrator-defined
custom roles, resolved in one place (`vyasa_core::user::effective_caps`).
[SECURITY.md](SECURITY.md) describes the whole model.

## Data model (summary)

- `posts` — type (post, page, block, or a custom type), status
  (draft/scheduled/published/private/trash), slug, **content as JSONB
  blocks** (`BlockDocument v1`, see [BLOCKS.md](BLOCKS.md)), typed meta.
- `content_types` / `content_fields` — administrator-defined post types and
  typed fields (text, number, date, choice, url, media, entry …); values live
  in `posts.meta.fields` and are validated on every write. Plugins can
  declare post types too.
- `post_revisions` — adaptive pruning (dense recent, sparse old).
- `terms` / `term_relationships` — normalized taxonomy.
- `comments` — threaded.
- `media` — metadata; bytes on FS or S3; derived sizes; blurhash.
- `users`, `roles`, `sessions`, `api_keys` (scoped capabilities) — argon2,
  RBAC, custom roles, optional public registration with email confirmation.
- `options` — typed JSONB site settings.
- `themes` — versioned rows: tokens + layout (+ optional Tera).
- `plugins` — wasm bytes, capabilities, enabled, version history.
- `jobs`, `webhooks` (HMAC-signed), `ai_logs` (usage/cost).

## Theme system

Themes are **data**: `tokens.json` (schema-validated design tokens → CSS
custom properties), `layout.json` (block composition per template type),
optional sandboxed Tera templates. Packaged as `.vytheme` (zip + manifest).
Versioned with rollback; child themes override tokens/layout. AI theme
generation goes through the identical validation/preview/version pipeline.

## Plugin system

wasmtime runtime; plugins implement WIT contracts (`plugins/wit/`). Host
brokers capabilities (`db:read:posts`, `net:fetch:domain`) declared at install.
Priority-ordered hooks replace `do_action`/`apply_filters`. Fuel + memory
limits; crash isolation; hot load/unload; signed updates.

## APIs

Shared service layer in `core`; REST and GraphQL are thin adapters (no logic
drift). OpenAPI spec at `/api/openapi.json` → generated TS client for admin.
GraphQL subscriptions for live preview. HMAC webhooks on publish events for
SSG rebuilds.

## Admin

A React single-page app (`admin/`: Vite, TypeScript, TanStack Router and
Query, shadcn/ui). Its API types are generated from the server's OpenAPI
document (`admin/openapi.json`, kept in sync by a test). The server serves
the built app from `admin/dist` under `/admin`.

## Data and migrations

Migrations are numbered SQL files in `crates/db/migrations/`, embedded in the
binary and applied by `vyasa migrate` and on start. They are
**forward-only**: back up the database before upgrading, because an older
binary will not start on a newer schema.

## Testing

Tests that touch the database get their own Postgres database, cloned from a
migrated template (`crates/testkit`); `scripts/test-with-db.sh` starts a
throwaway Postgres and runs the whole suite. Integration tests start the real
server with `TestServer`. Committed artefacts — `ROUTE-ACCESS.md`, the GraphQL
schema, the OpenAPI document — each have a test that fails when they drift
from the code.

## Deployment

One server binary plus PostgreSQL; the admin's built files sit next to it.
Media on the local filesystem or S3; SMTP for mail; both optional. See
[DEPLOYMENT.md](DEPLOYMENT.md) and [OPERATIONS.md](OPERATIONS.md).
