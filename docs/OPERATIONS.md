# Operations

Running Vyasa in production: what it exposes, what to watch, and what to do
when something is wrong.

See `docs/DEPLOYMENT.md` for installing and upgrading, and
`docs/PERFORMANCE.md` for the benchmark baseline.

## Site health

`GET /api/v1/site-health`, or the **Site health** page in the admin. Requires
the `manage_options` capability.

Seven read-only checks, each bounded at five seconds so a wedged dependency
shows up as a failed check rather than a hung request:

| Check | Reports |
|---|---|
| `database` | Reachability and the applied migration version |
| `storage` | That the media directory accepts an actual write, not just that it exists |
| `search_index` | Indexed document count against published posts |
| `job_queue` | Dead-lettered jobs, and how many are waiting |
| `plugins` | Plugins the runtime has marked errored or degraded |
| `ai_budget` | Spend against the configured monthly cap |
| `site_url` | Whether the public base URL is set |

Each check is `ok`, `warn` or `fail`, and the report takes the worst.
A search index that is merely behind is a `warn`, not a `fail`: search still
works through the SQL fallback.

## Metrics

Prometheus exposition at `GET /metrics`, on the main port and gated behind
`manage_options` — a scrape reveals traffic shape and queue state, which is
not public. Point your scraper at it with an admin session cookie or an API
key holding that capability.

| Family | Type | Labels |
|---|---|---|
| `vyasa_http_requests_total` | counter | `route`, `status` |
| `vyasa_http_request_duration_seconds` | histogram | `route` |
| `vyasa_render_cache_hits_total` | counter | — |
| `vyasa_render_cache_misses_total` | counter | — |
| `vyasa_db_pool_connections` | gauge | `state` (`total`, `idle`) |
| `vyasa_jobs_queue_depth` | gauge | `status` (`queued`, `running`, `dead`) |
| `vyasa_plugin_dispatch_failures_total` | counter | — |
| `vyasa_webhook_deliveries_total` | counter | — |
| `vyasa_emails_queued_total` | counter | — |

The `route` label is the template of the route that matched
(`/api/v1/posts/{id}`, `/category/{slug}`, `/{type}/{slug}`), never the
path that was requested, so the number of series cannot grow with the
number of posts or with whatever paths a client sends. A request no route
matched (the public site's themed 404, an unknown `/api/v1` path) is
`unmatched`. Rate-limited requests (`429`) are counted under the route they
asked for. As a backstop the registry keeps at most 2,000 route+status
series; past that, new ones are counted under `overflow`.

Duration buckets are 5 ms, 10, 25, 50, 100, 250 ms, 1 s, 5 s. A p99 pinned
at the top means requests are landing in `+Inf`.

### Dashboards and alerts

`ops/grafana/vyasa-overview.json` imports into Grafana; pick your Prometheus
data source when prompted. `ops/prometheus/vyasa-alerts.yml` holds sample
rules — thresholds assume a single-node install serving a modest site, so
tune the `for` durations before putting them on a pager.

A test asserts these files only reference families the build actually emits.
A panel naming a renamed series shows an empty graph rather than an error,
so drift is otherwise invisible.

## Logs

Structured via `tracing-subscriber`. `VYASA_LOG__LEVEL` sets the level
(`info` by default) and `VYASA_LOG__FORMAT` chooses `pretty` or `json`.

Every request runs inside a span carrying `request_id`, `method` and `path`.
An incoming `X-Request-Id` is reused when present — so a trace started at
your proxy stays one thread through the logs — and generated otherwise. The
value is echoed back on the response, which is what makes a user's report
correlatable with a log line.

Client-supplied ids are bounded to 128 ASCII characters; anything else is
replaced rather than logged.

## The job queue

One table, `jobs`, worked by `VYASA_JOBS__WORKERS` workers using
`FOR UPDATE SKIP LOCKED`. Four kinds run today: `media_derivatives`,
`publish_due`, `webhook_deliver` and `send_email`.

Failures retry with exponential backoff plus jitter and dead-letter after
five attempts. **Dead jobs never retry on their own.** To see them:

```sql
SELECT id, kind, attempts, last_error, created_at
FROM jobs WHERE status = 'dead' ORDER BY created_at DESC;
```

To retry one after fixing the cause:

```sql
UPDATE jobs SET status = 'queued', attempts = 0, run_at = now() WHERE id = $1;
```

## Common situations

**Search results are stale or missing.** The indexer subscribes to the event
bus, and a burst can make it lag — it logs when that happens. Rebuild with
`vyasa search reindex`. Tantivy allows one writer, so run it while the
server is stopped, or use `POST /api/v1/search/reindex` to rebuild in-process.

**Webhooks are not arriving.** Check `/api/v1/webhooks/{id}/deliveries` for
the recorded attempts and response codes. A non-2xx response counts as a
failure and is retried. Also confirm the subscription names an event the
server emits — unknown names are now rejected at creation, but subscriptions
made before that check may still hold one.

**Emails are not being sent.** With no `VYASA_SMTP__*` configured, the worker
logs the message and reports success. That is the development default; in
production it means password resets silently do not arrive. `site-health`
does not check this — the log line says `log-only mode`.

**A plugin is degraded.** A plugin whose hook fails is marked degraded and
skipped for subsequent hooks; the count is on `/metrics` and the state on the
admin Plugins page. Disable it, or fix and re-enable.

**Pages are stale.** The render cache is purged by domain events (publish,
edit, trash, restore, delete, new comment) and directly on theme activation
and option writes. If the invalidator lags it purges everything rather than
guess, which is safe but shows as a hit-rate dip.

**A field delete or kind change takes a while.** Deleting a field, changing
its kind, or flipping a content type between public and not public
reindexes every entry of that type for search, inline, inside the request.
On `post` or `page` of a large site that is the whole blog; do it at a
quiet time, or follow it with `vyasa search reindex` if the request was cut
off.

**Run a single instance.** The content-type registry (which slugs parse
as a type, which are public, which have an archive) is loaded at start and
kept in each process's memory, and type and field changes are serialised by
an in-process lock. A second instance against the same database does not
see a type another instance creates, deletes or flips between public and
not public: it keeps serving the old set (a deleted type's URLs, a
non-public type's entries) until it restarts. Vyasa supports one instance
per database; if another one runs anyway (a blue/green switch, a stray
process), restart it after any content type is created, deleted or changes
visibility. The database still refuses duplicate slugs and keys across
instances.

**Address-keyed limits behave oddly for non-ASCII addresses.** The
per-address mail buckets and the sign-in lockout fold addresses the way
PostgreSQL's `lower()` does under a non-C UTF-8 locale. Create the database
with a UTF-8 locale such as `en_US.UTF-8` (not `C`, not an ICU collation);
under another locale some spellings the database treats as one account land
in different buckets.

## The audit log

`GET /api/v1/audit-log?limit=100` (needs `manage_options`) returns recent
destructive administrative actions, newest first: user deletion and role
changes, theme activation, plugin lifecycle, and option writes.

It answers "who changed this and when". It is not a general activity feed —
ordinary content edits are not recorded there, because revisions already
carry that history.

Separately, `plugin_audit` records capability-broker decisions made by
plugins (denials, outbound fetches, quota hits).

## Backups

```bash
pg_dump "$VYASA_DATABASE_URL" > backup.sql
```

Take one before every migration. Migrations are forward-only — there is no
down path, so the backup is the rollback plan.

The media directory and the search index are separate from the database. The
index can be rebuilt from the database; uploaded media cannot, so it needs
backing up too.
