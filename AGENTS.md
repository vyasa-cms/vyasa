# Guide for AI coding assistants

Read [CONTRIBUTING.md](CONTRIBUTING.md) first: its architecture rules and
checks apply to every change, whoever writes it.

The short version:

- Crate layering: `common ← db ← core ← {themes, plugins, ai, search, jobs} ← api`. Lower crates never depend on higher ones.
- SQL lives only in `vyasa-db`. Authorization decisions live only in `crates/api/src/policy.rs`. REST routes are registered only through `authz::Guarded` with a declared `Access`.
- Tests that touch the database need `VYASA_TEST_DATABASE_URL`; `scripts/test-with-db.sh` provides a throwaway Postgres. Never point tests at a database holding real data.
- Before finishing: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, the tests, and for admin changes `pnpm lint && pnpm typecheck && pnpm test`.
- Regenerate committed artefacts when you change what they describe: `docs/ROUTE-ACCESS.md`, `docs/graphql-schema.graphql`, `admin/openapi.json` (commands in CONTRIBUTING.md).
- Never weaken a test or a gate to make it pass, and never commit secrets.
