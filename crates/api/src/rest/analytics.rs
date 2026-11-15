//! The analytics summary the dashboard reads.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};
use vyasa_db::repo::audience::dim;

use crate::error::{ApiErrorBody, ApiResult};
use crate::state::AppState;

/// Range selector.
#[derive(Deserialize, utoipa::IntoParams, Default)]
pub struct SummaryQuery {
    /// Days to cover (default 30, max 365).
    pub days: Option<i32>,
}

/// One day on the chart.
#[derive(Serialize, utoipa::ToSchema)]
pub struct DayViews {
    /// Calendar day, `YYYY-MM-DD`.
    pub day: String,
    /// Views that day.
    pub views: i64,
}

/// One row of a top list.
#[derive(Serialize, utoipa::ToSchema)]
pub struct TopEntry {
    /// Path or referrer host.
    pub name: String,
    /// Views.
    pub views: i64,
}

/// Everything the dashboard card needs in one call.
#[derive(Serialize, utoipa::ToSchema)]
pub struct AnalyticsSummary {
    /// Views today (UTC).
    pub today: i64,
    /// Views over the whole range.
    pub total: i64,
    /// Daily series, oldest first.
    pub series: Vec<DayViews>,
    /// Most-viewed paths.
    pub top_paths: Vec<TopEntry>,
    /// Busiest external referrer hosts.
    pub top_referrers: Vec<TopEntry>,
    /// Per day: views, cookieless unique visitors and bot hits.
    pub traffic: Vec<DayTraffic>,
    /// Unique visitors over the range (the sum of each day's count).
    pub visitors: i64,
    /// Requests counted as bots over the range; they are not part of `total`.
    pub bots: i64,
    /// The busiest paths with the same-length period before, for a trend.
    pub trends: Vec<PathTrend>,
    /// Device classes.
    pub devices: Vec<CountEntry>,
    /// Browser families.
    pub browsers: Vec<CountEntry>,
    /// `utm_source` values.
    pub utm_sources: Vec<CountEntry>,
    /// `utm_medium` values.
    pub utm_mediums: Vec<CountEntry>,
    /// `utm_campaign` values.
    pub utm_campaigns: Vec<CountEntry>,
    /// First pages of visits.
    pub entry_pages: Vec<CountEntry>,
    /// 404 paths; `detail` is the referrer host.
    pub not_found: Vec<CountEntry>,
    /// Internal search terms.
    pub searches: Vec<CountEntry>,
    /// Clicks through `/go/<name>`.
    pub outbound: Vec<CountEntry>,
    /// Conversions (forms, newsletter, `/go/` links marked as one).
    pub conversions: Vec<CountEntry>,
}

/// One day of traffic.
#[derive(Serialize, utoipa::ToSchema)]
pub struct DayTraffic {
    /// Calendar day, `YYYY-MM-DD`.
    pub day: String,
    /// Human page views.
    pub views: i64,
    /// Cookieless unique visitors.
    pub visitors: i64,
    /// Bot hits.
    pub bots: i64,
}

/// A path's views now against the period before.
#[derive(Serialize, utoipa::ToSchema)]
pub struct PathTrend {
    /// Site-relative path.
    pub path: String,
    /// Views over the range.
    pub views: i64,
    /// Views over the same number of days before it.
    pub previous: i64,
}

/// A counted value of one dimension.
#[derive(Serialize, utoipa::ToSchema)]
pub struct CountEntry {
    /// The value (a device class, a search term, a path ...).
    pub name: String,
    /// A second part, empty unless the dimension has one (a 404's referrer host).
    pub detail: String,
    /// The count.
    pub count: i64,
}

fn entries(rows: Vec<(String, String, i64)>) -> Vec<CountEntry> {
    rows.into_iter()
        .map(|(name, detail, count)| CountEntry {
            name,
            detail,
            count,
        })
        .collect()
}

/// `GET /api/v1/analytics/summary` — cookieless view counts.
///
/// # Errors
///
/// 401 when unauthenticated; 403 below Editor (`EditOthers`).
#[utoipa::path(
    get, path = "/api/v1/analytics/summary", tag = "analytics",
    security(("session_cookie" = [])),
    params(SummaryQuery),
    responses(
        (status = 200, description = "View summary", body = AnalyticsSummary),
        (status = 401, description = "Not authenticated", body = ApiErrorBody),
    )
)]
pub async fn summary(
    State(state): State<AppState>,
    Query(query): Query<SummaryQuery>,
) -> ApiResult<Json<AnalyticsSummary>> {
    // The route asks for `edit_others`: site-wide traffic (every path,
    // every referrer) is an editor's view, not something a subscriber's
    // login should open.
    let days = query.days.unwrap_or(30).clamp(1, 365);
    let series = state.audience.daily_views(days).await?;
    let today = chrono::Utc::now().date_naive();
    let today_views = series
        .iter()
        .find(|(d, _)| *d == today)
        .map_or(0, |(_, v)| *v);
    let total = series.iter().map(|(_, v)| v).sum();
    let top_paths = state.audience.top_paths(days, 10).await?;
    let top_referrers = state.audience.top_referrers(days, 10).await?;
    let traffic = state.audience.daily_traffic(days).await?;
    let (visitors, bots) = traffic
        .iter()
        .fold((0, 0), |(v, b), d| (v + d.visitors, b + d.bots));
    let trends = state.audience.path_trends(days, 10).await?;
    let top = |d: &'static str| {
        let audience = state.audience.clone();
        async move { audience.top_of(d, days, 10).await.map(entries) }
    };
    Ok(Json(AnalyticsSummary {
        visitors,
        bots,
        traffic: traffic
            .into_iter()
            .map(|d| DayTraffic {
                day: d.day.to_string(),
                views: d.views,
                visitors: d.visitors,
                bots: d.bots,
            })
            .collect(),
        trends: trends
            .into_iter()
            .map(|(path, views, previous)| PathTrend {
                path,
                views,
                previous,
            })
            .collect(),
        devices: top(dim::DEVICE).await?,
        browsers: top(dim::BROWSER).await?,
        utm_sources: top(dim::UTM_SOURCE).await?,
        utm_mediums: top(dim::UTM_MEDIUM).await?,
        utm_campaigns: top(dim::UTM_CAMPAIGN).await?,
        entry_pages: top(dim::ENTRY).await?,
        not_found: top(dim::NOT_FOUND).await?,
        searches: top(dim::SEARCH).await?,
        outbound: top(dim::OUTBOUND).await?,
        conversions: top(dim::CONVERSION).await?,
        today: today_views,
        total,
        series: series
            .into_iter()
            .map(|(day, views)| DayViews {
                day: day.to_string(),
                views,
            })
            .collect(),
        top_paths: top_paths
            .into_iter()
            .map(|(name, views)| TopEntry { name, views })
            .collect(),
        top_referrers: top_referrers
            .into_iter()
            .map(|(name, views)| TopEntry { name, views })
            .collect(),
    }))
}

/// A redirect of the `/go/<name>` table.
#[derive(Serialize, Deserialize, utoipa::ToSchema)]
pub struct LinkBody {
    /// Absolute `http` or `https` target.
    pub url: String,
    /// Whether a click also counts as a conversion.
    #[serde(default)]
    pub conversion: bool,
}

/// One row of the redirect table.
#[derive(Serialize, utoipa::ToSchema)]
pub struct LinkEntry {
    /// The name after `/go/`.
    pub name: String,
    /// The target.
    pub url: String,
    /// Whether a click counts as a conversion.
    pub conversion: bool,
}

/// `GET /api/v1/analytics/links` — the `/go/<name>` redirect table.
///
/// # Errors
///
/// 401 when unauthenticated; 403 below Editor.
#[utoipa::path(
    get, path = "/api/v1/analytics/links", tag = "analytics",
    security(("session_cookie" = [])),
    responses(
        (status = 200, description = "Redirects", body = Vec<LinkEntry>),
        (status = 401, description = "Not authenticated", body = ApiErrorBody),
    )
)]
pub async fn links(State(state): State<AppState>) -> ApiResult<Json<Vec<LinkEntry>>> {
    Ok(Json(
        state
            .audience
            .links()
            .await?
            .into_iter()
            .map(|l| LinkEntry {
                name: l.name,
                url: l.url,
                conversion: l.conversion,
            })
            .collect(),
    ))
}

/// `PUT /api/v1/analytics/links/{name}` — create or replace a redirect.
///
/// # Errors
///
/// 422 for a name outside `a-z0-9-_` or a target that is not an absolute http(s) URL; 403 without
/// `ManageOptions`.
#[utoipa::path(
    put, path = "/api/v1/analytics/links/{name}", tag = "analytics",
    security(("session_cookie" = [])),
    params(("name" = String, Path, description = "Link name")),
    request_body = LinkBody,
    responses(
        (status = 204, description = "Saved"),
        (status = 422, description = "Bad name or target", body = ApiErrorBody),
    )
)]
pub async fn put_link(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(body): Json<LinkBody>,
) -> ApiResult<StatusCode> {
    if !crate::public::go::valid_link_name(&name) {
        return Err(vyasa_common::AppError::validation(
            "a link name is 1 to 64 characters of a-z, 0-9, - and _",
        )
        .into());
    }
    if !crate::public::go::safe_target(&body.url) {
        return Err(vyasa_common::AppError::validation(
            "the target must be an absolute http or https URL",
        )
        .into());
    }
    state
        .audience
        .set_link(&name, &body.url, body.conversion)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE /api/v1/analytics/links/{name}` — remove a redirect.
///
/// # Errors
///
/// 403 without `ManageOptions`.
#[utoipa::path(
    delete, path = "/api/v1/analytics/links/{name}", tag = "analytics",
    security(("session_cookie" = [])),
    params(("name" = String, Path, description = "Link name")),
    responses((status = 204, description = "Deleted"))
)]
pub async fn delete_link(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> ApiResult<StatusCode> {
    state.audience.delete_link(&name).await?;
    Ok(StatusCode::NO_CONTENT)
}
