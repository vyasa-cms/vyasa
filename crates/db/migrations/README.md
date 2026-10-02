# Migrations

Applied in filename order by the embedded `sqlx` migrator. Names follow
`NNNN_description.sql`.

## A new file needs a rebuild of `vyasa-db`

`sqlx::migrate!` embeds these at **compile time**. A file that did not
exist when `vyasa-db` was last built is not one of that build's
dependencies, so cargo sees nothing to redo and the new migration is
silently absent from the binary — including from `cargo test`, where it
shows up as a constraint or column that "should" exist and does not.

After adding a file:

```bash
touch crates/db/src/migrate.rs   # or: cargo clean -p vyasa-db
```

## The gap at 0014

There is no `0014_*.sql`. The number was consumed during development and the
file never landed; the sequence runs 0013 → 0015.

**Do not fill it in.** `sqlx` records applied migrations by version number and
rejects a migration whose version is lower than one already applied, so adding
a `0014` now would fail on every database that is already past `0015` —
including production. The gap is cosmetic; the next migration is `0021`.
