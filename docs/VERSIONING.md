# Versioning and support

## What the version number promises

Vyasa follows [semantic versioning](https://semver.org). From 1.0.0 onward,
the version number is a statement about these four surfaces:

| Surface | Covered by semver |
|---|---|
| REST API (`/api/v1/*`) | Yes |
| GraphQL schema | Yes — see `docs/graphql-schema.graphql` |
| Plugin WIT contract (`crates/plugins/wit/host.wit`) | Yes |
| Theme package format and token schema | Yes |
| Rust crate APIs (`vyasa-core`, `vyasa-db`, …) | **No** — internal |
| Database schema | **No** — migrations own it |
| Admin SPA internals | **No** |

The crates are published as a workspace at a single version but are not a
supported library. Depending on `vyasa-core` directly means accepting that a
patch release can change it.

## What each bump means

**Patch** (1.0.0 → 1.0.1) — bug fixes. No new endpoints, fields, capabilities
or template variables. A theme or plugin that worked before works after.

**Minor** (1.0.0 → 1.1.0) — additions. New endpoints, optional request fields,
new GraphQL fields, new block kinds, new hook points, new template variables.
Existing integrations keep working; new ones may not work on older servers.

**Major** (1.0.0 → 2.0.0) — removals and changed meanings. A removed endpoint,
a field that changes type, a WIT contract revision, a token that is
interpreted differently.

## Deprecation

Anything covered above gets one minor release of warning before removal in
the next major. A deprecated REST endpoint keeps working and gains a
`Deprecation` response header; a deprecated GraphQL field is marked
`@deprecated` in the schema; a deprecated hook point still fires.

## Database migrations

Migrations are **forward-only**. There is no down path, by design: a
down-migration that silently discards a column is worse than a restore from
backup, because it looks like it worked.

The upgrade sequence, including taking the backup that serves as the
rollback plan, is in `docs/DEPLOYMENT.md`. Rehearse it with
`scripts/upgrade-test.sh`, which runs against a database with content in it —
the case that actually breaks.

A binary older than the database's migration version is unsupported: roll
the binary forward, or restore the database from before the migration.

## Support

This is a single-maintainer project. There is no LTS branch and no security
backport commitment to older minors — fixes land on the current release line.
Security reports go through `docs/SECURITY.md`.

## Rules that keep upgrades safe

Two invariants exist because breaking them would brick an existing
install rather than merely inconvenience it.

**Token structs may only gain optional fields.** The theme documents are
deserialized with `deny_unknown_fields`, and most carry `default` — so a
*new* binary reads an *old* theme row correctly. Adding a field without a
default (`ColorSlot` and `FontFace` have none today) would make every
existing theme row unreadable the moment the new version boots.

**The same `deny_unknown_fields` makes downgrade unsafe.** Once a newer
version has written a new token field, an older binary refuses to parse
the active theme. Combined with forward-only migrations, that means:
rolling a binary back is only safe if nothing has been written since the
upgrade. `vyasa update` enforces this — its automatic rollback fires only
in the window before the new build has served a request, and points at
the database dump otherwise.

**Media derivative readers tolerate old shapes forever.** The derivative
map has grown (real resizes, per-size WebP twins); rows written by older
versions keep working and simply offer fewer variants.
