# Contributing to Vyasa

Thank you for helping. Vyasa grows the way WordPress did: through people who
fix what bothers them, document what confused them, and build the plugins and
themes they wish existed.

## Ways to help

- **Report bugs** with the steps to reproduce them (use the issue template).
- **Improve the docs** — if something took you a while to work out, write it
  down for the next person.
- **Build plugins and themes** and list them in the
  [marketplace](https://github.com/vyasa-cms/marketplace).
- **Write code** — fixes and features in the core.

## Before you start

Small fixes (typos, obvious bugs, missing tests): open a pull request
directly. Anything larger — a new feature, a change to an API, a new
dependency — open an issue or a
[Discussion](https://github.com/vyasa-cms/vyasa/discussions) first, so we can
agree on the approach before you spend time on it. Issues labelled
`good first issue` are a good place to begin.

## Setting up

You need Rust (the version pinned in `rust-toolchain.toml`; rustup installs it automatically), Node 22
with pnpm, and Docker.

```bash
git clone https://github.com/vyasa-cms/vyasa && cd vyasa

# Postgres for development, on a port that will not clash with a local one
docker run -d --name vyasa-dev-db -p 127.0.0.1:55432:5432 \
  -e POSTGRES_USER=vyasa -e POSTGRES_PASSWORD=vyasa -e POSTGRES_DB=vyasa \
  postgres:17-alpine

# The server reads vyasa.toml and VYASA_* environment variables (not .env)
export VYASA_DATABASE_URL=postgres://vyasa:vyasa@127.0.0.1:55432/vyasa
cargo run -p vyasa-api -- migrate
cargo run -p vyasa-api -- serve           # the site on :3000; prints a setup token

cd admin && pnpm install && pnpm dev      # the admin, with hot reload
```

Open the admin, go to `/admin/setup`, and enter the token the server printed
to create your first account.

## Running the checks

These are the main checks CI runs (it also runs `cargo deny check`,
`scripts/scan-secrets.sh`, `scripts/check-links.sh`, `pnpm audit` and a
production admin build). A pull request needs all of them green.

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
scripts/test-with-db.sh                   # starts a throwaway Postgres, runs every test
(cd admin && pnpm lint && pnpm typecheck && pnpm test)
scripts/check-public-tree.sh
```

Tests that touch the database fail without `VYASA_TEST_DATABASE_URL`;
`scripts/test-with-db.sh` sets it for you. Each test gets its own database
cloned from a migrated template. Never point that variable at a database
holding real data.

## Architecture rules every change follows

[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) explains the design. The rules
below are enforced in review (and several by tests):

- **Crate layering is one-way:**
  `common ← db ← core ← {themes, plugins, ai, search, jobs} ← api`.
  `core` never imports HTTP or GraphQL types; `db` holds no domain rules;
  only `api` depends on `axum` and `async-graphql`.
- **SQL lives only in `vyasa-db`.**
- **Authorization decisions live only in `crates/api/src/policy.rs`.** A
  handler asks policy; it never decides ownership or visibility inline.
- **REST routes are registered only through `authz::Guarded`** with an
  explicit `Access`. Changing a route's access changes
  `docs/ROUTE-ACCESS.md`; regenerate it with
  `UPDATE_ROUTE_ACCESS=1 cargo test -p vyasa-api --bin vyasa route_access_table_is_committed`.
- **Themes are data**, never executable code, and all rendered HTML passes
  the sanitiser.
- **Migrations** are new files under `crates/db/migrations/` named
  `NNNN_description.sql`, and must be safe to run on a live database with
  data. They are forward-only.
- **Never weaken a test or a gate to make it pass.** Fix the code.
- Rust conventions: `#![forbid(unsafe_code)]`, clippy pedantic clean, doc
  comments on public items, no `unwrap` in library code.

## Changing the API

The REST and GraphQL schemas are committed so clients and the admin can rely
on them. When you change either, regenerate and commit:

```bash
cargo test -p vyasa-api --test graphql_sdl -- --ignored        # docs/graphql-schema.graphql
cargo test -p vyasa-api --test openapi_snapshot -- --ignored   # admin/openapi.json
(cd admin && VYASA_API_URL=http://127.0.0.1:9 pnpm gen:api)    # admin/src/api/schema.d.ts
```

## Commits and pull requests

- One logical change per pull request.
- Commit messages use conventional prefixes, as in the history:
  `feat(api): …`, `fix(core): …`, `docs: …`, `ci: …`, `chore: …`.
- Add a line to `CHANGELOG.md` under **Unreleased** for anything a user or
  plugin author would notice.

## Licence of contributions

Vyasa is dual-licensed under MIT or Apache-2.0. Unless you explicitly state
otherwise, any contribution intentionally submitted for inclusion in the work
by you, as defined in the Apache-2.0 license, shall be dual licensed as
above, without any additional terms or conditions.

## Conduct

Everyone taking part follows the [Code of Conduct](CODE_OF_CONDUCT.md).
