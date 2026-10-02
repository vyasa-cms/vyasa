# Performance

Recorded numbers, how they were produced, and what to do when they move.

## Reference machine

Everything below was measured on:

| | |
|---|---|
| CPU | Intel Core i7-9750H @ 2.60 GHz, 12 threads |
| Memory | 61 GB |
| Rust | 1.97.1, `--release` (`lto = "thin"`, `codegen-units = 1`) |
| OS | Linux 7.0 |
| Postgres | local, same host as the server |

Numbers from a different machine are not comparable in absolute terms. What
travels between machines is the *shape*: the ratio between routes, and the
direction a change moves them.

## Micro benchmarks

`cargo bench -p vyasa-themes -p vyasa-common`. Criterion reports a
confidence interval; the middle value is quoted.

| Benchmark | Time | Notes |
|---|---:|---|
| `render_blocks/3` | 43 µs | A short post — the common case |
| `render_blocks/20` | 226 µs | A typical article |
| `render_blocks/100` | 1.16 ms | Long-form; scales linearly, as expected |
| `tokens_to_css` | 5.5 µs | Runs once per page, negligible |
| `sanitize_comment_html` | 298 µs | 20 hostile comments; per comment ≈ 15 µs |
| `block_document_to_json` | 7.0 µs | Serialization on save |
| `block_document_from_json` | 19 µs | Deserialization on every uncached render |
| `slugify/plain` | 118 ns | |
| `slugify/accented` | 389 ns | Transliteration path |
| `slugify/long` | 3.7 µs | 700-character input |

Block rendering scales linearly with block count (3 → 20 → 100 blocks costs
roughly 43 µs → 226 µs → 1.16 ms), which is what a per-block loop should do.
There is no superlinear term to hunt.

## What the render cache is worth

An uncached page render is dominated by `render_blocks` plus deserialization
— on the order of 250 µs of CPU for a typical post, before the database
query. A cache hit skips all of it and serves from memory.

The hit rate is exported at `/metrics`:

```
vyasa_render_cache_hits_total / (vyasa_render_cache_hits_total + vyasa_render_cache_misses_total)
```

On a site with steady traffic this should sit above 90%. Below 50%
sustained means something is purging more than it should — the invalidator
drops pages on every post publish, edit, trash, restore, delete and new
comment, so a site that publishes constantly will legitimately sit lower.

## Load tests

`perf/load.sh` drives a running release build against a seeded database.
See `perf/README.md` for setup. The route set and profiles are defined
there; results are not recorded in this file because they depend on the
database size and the machine's other load, and a stale table is worse than
no table.

Record a baseline for **your** deployment before tuning it, and compare
against that.

## Soak testing

`perf/soak.sh` holds steady traffic for an hour and samples RSS every 30
seconds into a TSV. A leak shows as RSS that climbs throughout and does not
plateau; ordinary caching shows as a rise to a plateau. The render cache is
bounded by total bytes, so a plateau is the expected shape.

## Startup

The server's own budget is under two seconds to first request, dominated by
migration checks and theme bootstrap. `vyasa health` measures the database
leg on its own.

## When a number moves

1. Re-run on an otherwise-idle machine. Criterion's own "performance has
   regressed" line compares against the *previous run in the same working
   directory*, so it fires on noise and on unrelated changes.
2. Check whether the change is in the benchmark's own inputs — a longer
   default document makes everything slower without anything being wrong.
3. A change above roughly 10% that survives a re-run is worth a look.
   Below that is usually the page cache or CPU frequency scaling.
