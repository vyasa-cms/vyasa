# Vyasa Tech Stack

| Concern | Choice | Notes |
|---|---|---|
| Language | Rust (edition 2021, stable ≥1.85) | `#![forbid(unsafe_code)]` workspace-wide |
| Async runtime | tokio | |
| HTTP framework | axum | only in `api` crate |
| REST docs | utoipa + utoipa-swagger-ui | OpenAPI → TS client codegen |
| GraphQL | async-graphql | queries, mutations, subscriptions |
| Database | PostgreSQL | only hard external dependency |
| DB access | sqlx | compile-time checked queries, embedded migrations |
| Migrations | sqlx migrate (`crates/db/migrations/NNNN_*.sql`) | auto-applied on boot |
| Auth | argon2, tower-sessions, JWT (headless api_keys) | RBAC with granular caps |
| Templates | tera (sandboxed) | theme rendering |
| HTML sanitization | ammonia | every content render path |
| WASM plugins | wasmtime + WIT (component model) | fuel + memory limits, capabilities |
| Search | tantivy | BM25; index dir on local FS |
| Cache | moka | keyed by (theme_version, post_revision) |
| Images | image + blurhash | derived sizes in background jobs |
| Email | lettre (SMTP) | queued |
| Jobs | tokio-cron-scheduler + Postgres-backed queue | real cron, replaces wp-cron |
| Config | config crate + env | `vyasa.toml` / `VYASA_*` |
| Observability | tracing + OpenTelemetry | |
| Validation | schemars + validator | tokens, meta, AI outputs |
| AI providers | Anthropic (primary), OpenAI (fallback) | structured output only, JSON-schema validated |
| Admin UI | Vite + React + TypeScript | |
| Admin framework | TanStack Router/Query/Table/Form | headless — pair with shadcn/ui |
| Admin styling | Tailwind CSS + shadcn/ui | |
| Admin tests | vitest + testing-library | |
| Frontend package manager | pnpm | |
| Static build | rust-embed | admin dist + starter themes embedded in binary |
| Release target | x86_64-unknown-linux-musl | scratch Docker image, single binary |

## Quality gates

```bash
# Rust (always)
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace

# Frontend (phases touching admin/)
pnpm lint && pnpm typecheck && pnpm test && pnpm build
```

## Toolchain

- rustup (rust-toolchain.toml pins stable + rustfmt + clippy)
- Node 22+, pnpm 9+
- PostgreSQL 16+ for development (`docker compose up db` once infra lands)
