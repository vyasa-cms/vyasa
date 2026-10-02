//! The analytics summary the dashboard reads.

use axum::extract::{Query, State};
use axum::Json;
use serde::{Deserialize, Serialize};

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
    Ok(Json(AnalyticsSummary {
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
