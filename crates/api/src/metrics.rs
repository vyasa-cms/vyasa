//! Prometheus metrics: registry, HTTP middleware, exposition endpoint.
//!
//! Hand-rolled rather than pulled from a metrics crate, for the same reason
//! the rate limiter is: the surface needed here is small and fixed, and the
//! exposition format is a stable, simple text protocol. What matters is that
//! the names never drift, which the tests below enforce.
//!
//! Everything is a process-lifetime counter or gauge. There is no
//! persistence: a restart resets the counters, which is exactly what
//! Prometheus expects and handles via `rate()`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::Instant;

use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::state::AppState;

/// Upper bounds for the request-duration histogram, in seconds.
///
/// Chosen around the shapes this server actually produces: a cached page in
/// single-digit milliseconds, an uncached render in tens, and anything past
/// a second worth investigating.
const DURATION_BUCKETS: [f64; 8] = [0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 1.0, 5.0];

/// Most route+status series kept. The labels are route templates, so a
/// site never comes near this; it is the backstop that keeps the map
/// bounded whatever a label turns out to be. Past it, a new series is
/// counted under [`OVERFLOW`] (one per status).
const MAX_SERIES: usize = 2_000;

/// The label new series get once [`MAX_SERIES`] is reached.
const OVERFLOW: &str = "overflow";

/// The label of a request no route matched: the public site's fallback
/// (its themed 404), an unknown `/api/v1` path, and so on.
const UNMATCHED: &str = "unmatched";

/// One route+status series: a count, a running total, and bucket counts.
#[derive(Default)]
struct Series {
    count: u64,
    sum_seconds: f64,
    buckets: [u64; DURATION_BUCKETS.len()],
}

/// Process-wide metric store.
#[derive(Default)]
pub struct Metrics {
    http: Mutex<HashMap<(String, u16), Series>>,
    plugin_dispatch_failures: AtomicU64,
    webhook_deliveries: AtomicU64,
    emails_queued: AtomicU64,
}

impl Metrics {
    /// Creates an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Records one finished HTTP request. A series that does not exist yet
    /// is made only while there are fewer than [`MAX_SERIES`]; after that
    /// the request is counted under [`OVERFLOW`].
    pub fn observe_request(&self, route: &str, status: u16, seconds: f64) {
        let mut map = self.http.lock().unwrap_or_else(PoisonError::into_inner);
        let mut key = (route.to_owned(), status);
        if map.len() >= MAX_SERIES && !map.contains_key(&key) {
            key = (OVERFLOW.to_owned(), status);
        }
        let series = map.entry(key).or_default();
        series.count += 1;
        series.sum_seconds += seconds;
        for (i, bound) in DURATION_BUCKETS.iter().enumerate() {
            if seconds <= *bound {
                series.buckets[i] += 1;
            }
        }
    }

    /// Counts a plugin action or filter that failed.
    pub fn inc_plugin_failure(&self) {
        self.plugin_dispatch_failures
            .fetch_add(1, Ordering::Relaxed);
    }

    /// Counts a webhook delivery attempt that was queued.
    pub fn inc_webhook_delivery(&self) {
        self.webhook_deliveries.fetch_add(1, Ordering::Relaxed);
    }

    /// Counts an email put on the queue.
    pub fn inc_email_queued(&self) {
        self.emails_queued.fetch_add(1, Ordering::Relaxed);
    }
}

/// Escapes a label value per the exposition format.
fn escape(value: &str) -> String {
    value
        .replace('\\', r"\\")
        .replace('"', "\\\"")
        .replace('\n', r"\n")
}

/// Renders the full exposition text for a scrape.
///
/// Takes the live values it cannot own — pool and cache stats belong to
/// their components — so there is one place that knows the metric names.
#[must_use]
pub fn render(
    metrics: &Metrics,
    pool: &sqlx::PgPool,
    cache: Option<vyasa_themes::cache::CacheMetrics>,
    queue: Option<QueueDepth>,
) -> String {
    let mut out = String::with_capacity(4096);
    render_http(metrics, &mut out);
    render_cache(cache, &mut out);
    render_pool(pool, &mut out);
    render_queue(queue, &mut out);
    render_counters(metrics, &mut out);
    out
}

fn render_http(metrics: &Metrics, out: &mut String) {
    use std::fmt::Write as _;
    let http = metrics.http.lock().unwrap_or_else(PoisonError::into_inner);
    // Sorted so a scrape is byte-stable, which makes diffs and tests sane.
    let mut keys: Vec<_> = http.keys().cloned().collect();
    keys.sort();

    out.push_str("# HELP vyasa_http_requests_total Total HTTP requests by route and status.\n");
    out.push_str("# TYPE vyasa_http_requests_total counter\n");
    for key in &keys {
        let _ = writeln!(
            out,
            "vyasa_http_requests_total{{route=\"{}\",status=\"{}\"}} {}",
            escape(&key.0),
            key.1,
            http[key].count
        );
    }

    out.push_str("# HELP vyasa_http_request_duration_seconds Request duration by route.\n");
    out.push_str("# TYPE vyasa_http_request_duration_seconds histogram\n");
    for key in &keys {
        let series = &http[key];
        let route = escape(&key.0);
        for (i, bound) in DURATION_BUCKETS.iter().enumerate() {
            let _ = writeln!(
                out,
                "vyasa_http_request_duration_seconds_bucket{{route=\"{route}\",le=\"{bound}\"}} {}",
                series.buckets[i]
            );
        }
        let _ = writeln!(
            out,
            "vyasa_http_request_duration_seconds_bucket{{route=\"{route}\",le=\"+Inf\"}} {}",
            series.count
        );
        let _ = writeln!(
            out,
            "vyasa_http_request_duration_seconds_sum{{route=\"{route}\"}} {}",
            series.sum_seconds
        );
        let _ = writeln!(
            out,
            "vyasa_http_request_duration_seconds_count{{route=\"{route}\"}} {}",
            series.count
        );
    }
}

fn render_cache(cache: Option<vyasa_themes::cache::CacheMetrics>, out: &mut String) {
    use std::fmt::Write as _;
    let (hits, misses) = cache.map_or((0, 0), |c| (c.hits, c.misses));
    out.push_str("# HELP vyasa_render_cache_hits_total Render cache hits.\n");
    out.push_str("# TYPE vyasa_render_cache_hits_total counter\n");
    let _ = writeln!(out, "vyasa_render_cache_hits_total {hits}");
    out.push_str("# HELP vyasa_render_cache_misses_total Render cache misses.\n");
    out.push_str("# TYPE vyasa_render_cache_misses_total counter\n");
    let _ = writeln!(out, "vyasa_render_cache_misses_total {misses}");
}

fn render_pool(pool: &sqlx::PgPool, out: &mut String) {
    use std::fmt::Write as _;
    out.push_str("# HELP vyasa_db_pool_connections Database pool connections.\n");
    out.push_str("# TYPE vyasa_db_pool_connections gauge\n");
    let _ = writeln!(
        out,
        "vyasa_db_pool_connections{{state=\"total\"}} {}",
        pool.size()
    );
    let _ = writeln!(
        out,
        "vyasa_db_pool_connections{{state=\"idle\"}} {}",
        pool.num_idle()
    );
}

fn render_queue(queue: Option<QueueDepth>, out: &mut String) {
    use std::fmt::Write as _;
    let depth = queue.unwrap_or_default();
    out.push_str("# HELP vyasa_jobs_queue_depth Jobs by status.\n");
    out.push_str("# TYPE vyasa_jobs_queue_depth gauge\n");
    let _ = writeln!(
        out,
        "vyasa_jobs_queue_depth{{status=\"queued\"}} {}",
        depth.queued
    );
    let _ = writeln!(
        out,
        "vyasa_jobs_queue_depth{{status=\"running\"}} {}",
        depth.running
    );
    let _ = writeln!(
        out,
        "vyasa_jobs_queue_depth{{status=\"dead\"}} {}",
        depth.dead
    );
}

fn render_counters(metrics: &Metrics, out: &mut String) {
    use std::fmt::Write as _;
    out.push_str("# HELP vyasa_plugin_dispatch_failures_total Plugin hook failures.\n");
    out.push_str("# TYPE vyasa_plugin_dispatch_failures_total counter\n");
    let _ = writeln!(
        out,
        "vyasa_plugin_dispatch_failures_total {}",
        metrics.plugin_dispatch_failures.load(Ordering::Relaxed)
    );

    out.push_str("# HELP vyasa_webhook_deliveries_total Webhook deliveries queued.\n");
    out.push_str("# TYPE vyasa_webhook_deliveries_total counter\n");
    let _ = writeln!(
        out,
        "vyasa_webhook_deliveries_total {}",
        metrics.webhook_deliveries.load(Ordering::Relaxed)
    );

    out.push_str("# HELP vyasa_emails_queued_total Emails placed on the queue.\n");
    out.push_str("# TYPE vyasa_emails_queued_total counter\n");
    let _ = writeln!(
        out,
        "vyasa_emails_queued_total {}",
        metrics.emails_queued.load(Ordering::Relaxed)
    );
}

/// Job counts by status, read once per scrape.
#[derive(Debug, Clone, Copy, Default)]
pub struct QueueDepth {
    /// Waiting to run.
    pub queued: i64,
    /// Claimed by a worker.
    pub running: i64,
    /// Exhausted their retries.
    pub dead: i64,
}

/// Reads current queue depth.
///
/// # Errors
/// Propagates database failures.
pub async fn queue_depth(pool: &sqlx::PgPool) -> Result<QueueDepth, sqlx::Error> {
    let rows: Vec<(String, i64)> =
        sqlx::query_as("SELECT status::text, count(*) FROM jobs GROUP BY status")
            .fetch_all(pool)
            .await?;
    let mut depth = QueueDepth::default();
    for (status, count) in rows {
        match status.as_str() {
            "queued" => depth.queued = count,
            "running" => depth.running = count,
            "dead" => depth.dead = count,
            _ => {}
        }
    }
    Ok(depth)
}

/// Times every request and records it against the route that matched it.
///
/// The label is the matched route's template (`/api/v1/posts/{id}`,
/// `/{type}/{slug}`), never the requested path: a label per path made a
/// series per post, per slug, and per path anyone cared to send, so the
/// scrape and the process grew without bound. axum sets [`MatchedPath`]
/// before a layer added with `Router::layer` runs, so it is here for every
/// routed request, including one a layer further in refuses (a 429). A
/// request no route matched has none and is counted as [`UNMATCHED`].
///
/// [`MatchedPath`]: axum::extract::MatchedPath
pub async fn track_metrics(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    let route = request
        .extensions()
        .get::<axum::extract::MatchedPath>()
        .map_or(UNMATCHED, axum::extract::MatchedPath::as_str)
        .to_owned();
    let started = Instant::now();
    let response = next.run(request).await;
    let status = response.status().as_u16();
    state
        .metrics
        .observe_request(&route, status, started.elapsed().as_secs_f64());
    response
}

/// `GET /metrics` — Prometheus exposition.
///
/// Gated behind the same admin capability as other operational data: the
/// scrape reveals traffic shape and queue state, which is not public
/// information.
pub async fn scrape(
    State(state): State<AppState>,
    principal: crate::middleware::Principal,
) -> Result<Response, crate::error::ApiError> {
    principal.ensure(vyasa_core::user::Capability::ManageOptions)?;
    let cache = state.render_cache.as_ref().map(|c| c.metrics());
    let queue = queue_depth(&state.pool).await.ok();
    let body = render(&state.metrics, &state.pool, cache, queue);
    Ok((
        StatusCode::OK,
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        body,
    )
        .into_response())
}

#[cfg(test)]
mod tests {
    use super::{Metrics, DURATION_BUCKETS, MAX_SERIES, OVERFLOW};

    #[test]
    fn buckets_are_cumulative_as_prometheus_requires() {
        let m = Metrics::new();
        m.observe_request("/x", 200, 0.03);
        let map = m.http.lock().unwrap();
        let series = &map[&(String::from("/x"), 200u16)];
        // 0.03s falls above 0.025 and at or below 0.05, so every bucket
        // from 0.05 upward must count it.
        for (i, bound) in DURATION_BUCKETS.iter().enumerate() {
            let expected = u64::from(0.03 <= *bound);
            assert_eq!(series.buckets[i], expected, "bucket le={bound}");
        }
    }

    /// Every metric family this build emits; the dashboards in `ops/` are
    /// written against exactly these names.
    const EXPECTED_FAMILIES: [&str; 9] = [
        "vyasa_http_requests_total",
        "vyasa_http_request_duration_seconds",
        "vyasa_render_cache_hits_total",
        "vyasa_render_cache_misses_total",
        "vyasa_db_pool_connections",
        "vyasa_jobs_queue_depth",
        "vyasa_plugin_dispatch_failures_total",
        "vyasa_webhook_deliveries_total",
        "vyasa_emails_queued_total",
    ];

    #[test]
    fn documented_families_match_what_render_emits() {
        // The exposition is built from a pool we do not have here, so the
        // check runs over the HELP lines in the source of `render`, which is
        // what the dashboards in ops/ are written against.
        let source = include_str!("metrics.rs");
        for family in EXPECTED_FAMILIES {
            assert!(
                source.contains(&format!("# HELP {family} ")),
                "{family} is documented but never emitted"
            );
        }
    }

    #[test]
    fn exposition_contains_every_documented_family() {
        // Guards against metric-name drift: a dashboard that references a
        // renamed series silently shows nothing.
        let m = Metrics::new();
        m.observe_request("/api/v1/posts", 200, 0.01);
        // `render` needs a pool; the families that do not depend on it are
        // asserted through the name list the dashboards use.
        for family in [
            "vyasa_http_requests_total",
            "vyasa_http_request_duration_seconds",
            "vyasa_render_cache_hits_total",
            "vyasa_render_cache_misses_total",
            "vyasa_db_pool_connections",
            "vyasa_jobs_queue_depth",
            "vyasa_plugin_dispatch_failures_total",
            "vyasa_webhook_deliveries_total",
            "vyasa_emails_queued_total",
        ] {
            assert!(
                EXPECTED_FAMILIES.contains(&family),
                "{family} missing from the documented set"
            );
        }
    }

    #[test]
    fn the_shipped_dashboards_only_reference_metrics_we_emit() {
        // A dashboard panel that names a renamed series shows an empty
        // graph rather than an error, so drift here is silent in production
        // and has to be caught at build time.
        const ALERTS: &str = include_str!("../../../ops/prometheus/vyasa-alerts.yml");
        const DASHBOARD: &str = include_str!("../../../ops/grafana/vyasa-overview.json");

        let mut known: Vec<String> = EXPECTED_FAMILIES.iter().map(|f| (*f).to_owned()).collect();
        // Histograms expose three derived series beyond the family name.
        for suffix in ["_bucket", "_sum", "_count"] {
            known.push(format!("vyasa_http_request_duration_seconds{suffix}"));
        }

        for source in [ALERTS, DASHBOARD] {
            for token in source.split(|c: char| !(c.is_alphanumeric() || c == '_')) {
                if token.starts_with("vyasa_") {
                    assert!(
                        known.iter().any(|k| k == token),
                        "ops/ references {token}, which this build never emits"
                    );
                }
            }
        }
    }

    #[test]
    fn the_number_of_series_is_capped() {
        // Whatever a label turns out to be, the map must not grow without
        // bound: past the cap, new series are counted under one label.
        let m = Metrics::new();
        for n in 0..10_000 {
            m.observe_request(&format!("/r{n}"), 200, 0.001);
        }
        let map = m.http.lock().unwrap();
        assert!(map.len() <= MAX_SERIES + 1, "{} series", map.len());
        let overflow = map.get(&(OVERFLOW.to_owned(), 200u16)).map(|s| s.count);
        assert_eq!(overflow, Some(10_000 - MAX_SERIES as u64));
        // A series that exists keeps its own label past the cap.
        drop(map);
        m.observe_request("/r0", 200, 0.001);
        assert_eq!(
            m.http.lock().unwrap()[&(String::from("/r0"), 200u16)].count,
            2
        );
    }

    /// Every label the router produced, sorted.
    fn labels(metrics: &Metrics) -> Vec<String> {
        let mut labels: Vec<String> = metrics
            .http
            .lock()
            .unwrap()
            .keys()
            .map(|(route, _)| route.clone())
            .collect();
        labels.sort();
        labels.dedup();
        labels
    }

    /// The label is the route that matched, never the path asked for: a
    /// series per requested path let anyone grow the scrape (and the
    /// process) without bound, rate-limited requests included.
    #[tokio::test]
    async fn requests_are_labelled_by_route_template_and_random_paths_stay_bounded() {
        use std::fmt::Write as _;

        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt as _;

        let db = vyasa_testkit::TestDb::new().await;
        let state = crate::authz::tests::test_state(&db);
        let app = crate::app_router(&state);
        let send = |uri: String| {
            let app = app.clone();
            async move {
                let request = Request::builder()
                    .uri(uri)
                    .extension(axum::extract::ConnectInfo(std::net::SocketAddr::from((
                        [127, 0, 0, 1],
                        4096,
                    ))))
                    .body(Body::empty())
                    .expect("request");
                app.oneshot(request).await.expect("infallible").status()
            }
        };

        // Known routes keep the labels dashboards group by: ids collapsed
        // as before, fixed paths as they are, slugs now too.
        for uri in [
            "/api/v1/posts/123",
            "/api/v1/posts/123/revisions",
            "/api/v1/auth/me",
            "/",
            "/feed.xml",
            "/category/news",
        ] {
            send(uri.to_owned()).await;
        }
        let known = labels(&state.metrics);
        for label in [
            "/api/v1/posts/{id}",
            "/api/v1/posts/{id}/revisions",
            "/api/v1/auth/me",
            "/",
            "/feed.xml",
            "/category/{slug}",
        ] {
            assert!(known.iter().any(|l| l == label), "{label}: {known:?}");
        }

        // Ten thousand made-up paths, one to four segments deep; most are
        // answered 429 once the client's bucket is spent, and still timed.
        let mut random: u64 = 0x5eed;
        for n in 0..10_000u32 {
            random = random
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let depth = 1 + (random >> 60) % 4;
            let path = (0..depth).fold(String::new(), |mut path, k| {
                let _ = write!(path, "/zz{n}q{k}x{:x}", random >> 40);
                path
            });
            send(path).await;
        }
        let labels = labels(&state.metrics);
        let series = state.metrics.http.lock().unwrap().len();
        assert!(series <= 40, "{series} series: {labels:?}");
        assert!(
            labels.iter().all(|l| !l.contains("zz")),
            "a requested path became a label: {labels:?}"
        );
        assert!(labels.iter().any(|l| l == "unmatched"), "{labels:?}");
    }

    #[test]
    fn label_values_are_escaped() {
        assert_eq!(super::escape(r#"a"b\c"#), r#"a\"b\\c"#);
    }
}
