# Feature matrix (as-built)

What is in the tree, verified against it on 2026-08-28 rather than against
the plan. "Partial" means a real limitation is documented in the relevant
phase doc, not that the feature is unfinished in a vague way.

## Content

| Feature | Status | Notes |
|---|---|---|
| Posts / pages / blocks | done | 25 block kinds, validated against a registry |
| Writing canvas | done | In-place editing of text, images (drop/paste/upload), tables, code, nested lists; auto-drafts; revision compare; the block wire format is `docs/BLOCKS.md` |
| Revisions & autosave | done | Working copies: Save never publishes a live post, Update does; signed preview tokens for unpublished content |
| Scheduling | done | Queue worker publishes when due |
| Taxonomy | done | Categories and tags, with merge |
| Content types & custom fields | done | Admin-created types (public URL, optional archive, per-type templates) and typed fields on any type — text, textarea, number, boolean, date, choice, url, media, entry; validated on every write, `required` on publish/schedule; served by REST/GraphQL, themes (`entry.fields`), bindings, search, export/import; `manage_options` to manage |
| Comments | done | Threading and moderation |
| Media | done | Local storage, derivatives, blurhash |
| WordPress import | done | Users, posts, terms, comments |

## Presentation

| Feature | Status | Notes |
|---|---|---|
| Themes | done | Tokens + layout + sandboxed Tera + `.vytheme` packages |
| Starter themes | done | blog, portfolio, docs, storefront-lite |
| Render cache | done | Purged by domain events; hit rate on `/metrics` |
| Feeds | done | RSS and Atom, from the configured site identity |
| Sitemap & robots | done | Absolute URLs from `site_url`, falling back to the request origin |
| Custom CSS | done | Sanitised and namespaced under `#vy-site` |

## Platform

| Feature | Status | Notes |
|---|---|---|
| Users / RBAC | done | 5 built-in roles + custom roles, 12 capabilities |
| Sessions | done | Argon2, HTTP-only cookie |
| Registration | done | Public self-registration, off by default; accounts start unconfirmed and cannot sign in until the mailbox confirms; generated usernames; default role capped at author-level power (never administrator or editor); a stored password survives confirmation only when the link holder types it; unconfirmed accounts purged after 7 days |
| API keys | done | Create / list / revoke; a key cannot exceed its creator's capabilities |
| Login lockout | done | Progressive per-`(account, IP)` delay |
| Rate limiting | done | In-process per-IP buckets (single node; not `tower_governor`) |
| CSRF & security headers | done | |
| Plugins | done | wasmtime components, capability broker, hook registry, actions, blocks, routes, cron, settings forms, assets, custom post types |
| REST API | done | OpenAPI at `/api/docs`, generated TypeScript types |
| GraphQL | done | Queries, mutations, subscriptions; SDL in `docs/graphql-schema.graphql` |
| Admin SPA | done | React + TanStack Router/Query, dark mode, mobile |

## Integrations

| Feature | Status | Notes |
|---|---|---|
| Search | done | Tantivy BM25 with highlighted snippets; REST, GraphQL and a public page; SQL fallback |
| Webhooks | done | HMAC-SHA256 signed, queued delivery, retries, delivery log |
| Email | done | Templated and queued via lettre; log-only with no relay configured |
| AI generation | done | Anthropic with OpenAI fallback, cost caps |
| AI integrations | done | Alt text, comment screening, excerpt/SEO suggest + autofill, embeddings with semantic search and related posts, image generation, transcription, read-aloud; all switchable, all off by default — `docs/AI-MODELS.md` |
| AI model registry | done | Anthropic, OpenAI, OpenRouter keys (sealed with `VYASA_SECRET_KEY`); models per kind with a default and failover; probes and catalogues — `docs/AI-MODELS.md` |
| Theme studio | done | Draft working copies, token/layout/template editors, live preview of any page, revisions and publish |
| Theme assistant | done | Optional: typed ops from a text model (vision for moodboards), validated like a human edit |
| AI editor assist | done | Titles, excerpts, SEO meta, tag suggestions; rewrite / shorten / expand / fix a selection and continue writing from the canvas |

## Operations

| Feature | Status | Notes |
|---|---|---|
| Metrics | done | Prometheus at `/metrics`, nine families, auth-gated |
| Site health | done | Seven bounded checks, REST endpoint and admin page |
| Dashboards | done | Grafana JSON and alert samples in `ops/` |
| Structured logs | done | `X-Request-Id` correlation, JSON or pretty |
| Job queue | done | Postgres-backed, backoff, dead-letter |
| Benchmarks | done | Criterion, with a baseline in `docs/PERFORMANCE.md` |
| Load tests | done | `perf/load.sh` and `perf/soak.sh` |
| Container | partial | debian-slim, not musl/scratch — wasmtime and Tantivy want a real libc |
| CI / release | done | Gates, dependency audit, tarball and GHCR image |
| Upgrade rehearsal | done | `scripts/upgrade-test.sh`, against a populated database |
| Distributed tracing | not built | Structured logs only; no OTLP exporter |

## Commerce & community (reference plugins, phase 55)

| Capability | WordPress equivalent | Vyasa | Notes |
|---|---|---|---|
| Products as content | WooCommerce products | ✅ `storefront` plugin | `product` post type; price/stock as plugin meta; designed store pages via `store/*` sections |
| Cart | WooCommerce cart (guest + user) | ◐ signed-in only | plugin routes never see cookies (sandbox boundary), so carts hang off accounts |
| Checkout / payments | WooCommerce gateways | ❌ order requests by mail | payments = future provider-plugin slot with its own security review |
| Forum topics | bbPress topics | ✅ `forum-lite` plugin | `topic` post type; signed-in creation via plugin route |
| Forum replies | bbPress replies | ✅ core comments | threading, moderation, spam hook, notifications all inherited |
| Activity/profiles | BuddyPress | ❌ out of scope | forum-lite is a topics surface, not a social network |

## Not built

| Feature | Notes |
|---|---|
| Multi-site | Post-1.0 |
| Plugin marketplace | Post-1.0 |
| Inbound email | Never core |
| Newsletters | Never core |
| Horizontal scaling | Single-node product; rate limiting and caches are in-process |
