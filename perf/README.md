# Load tests

Scripts for measuring the public site and the REST surface under load.

They deliberately drive a **running server against a seeded database**
rather than spinning one up themselves: the numbers only mean something if
the process under test is configured the way production is (release build,
render cache on, real Postgres).

## Setup

```bash
cargo build --release
./target/release/vyasa migrate
./target/release/vyasa admin create --email you@example.test
./target/release/vyasa dev seed --count 1000
./target/release/vyasa search reindex
./target/release/vyasa serve &
```

`dev seed` is deterministic, so two runs at the same `--count` produce the
same corpus and results are comparable.

## Running

```bash
./perf/load.sh                       # all routes, default profile
./perf/load.sh --profile heavy       # higher concurrency
./perf/load.sh --base https://host   # against a deployed instance
```

Requires [`oha`](https://github.com/hatoolab/oha) (`cargo install oha`).
The script checks for it and explains how to install it if missing.

## What is measured

| Route | Why it is in the set |
|---|---|
| `/` | The most-hit page; exercises the archive query and the render cache |
| `/post/{slug}` | Single render: blocks → HTML → sanitize |
| `/category/{slug}` | Archive pagination and the term join |
| `/search?s=…` | Tantivy query path, which bypasses the render cache |
| `/feed.xml` | XML generation over the same post query |
| `/api/v1/posts` | JSON list: serialization without any template work |

## Reading the results

`docs/PERFORMANCE.md` holds the recorded baseline. A change of more than
about 10% against it on the same machine is worth investigating; anything
less is usually noise from the page cache or CPU scaling.

Cache effectiveness is visible on the `/metrics` endpoint
(`vyasa_render_cache_hits_total` against `..._misses_total`) — a repeated
run over the same URLs should show a hit rate above 90%.
