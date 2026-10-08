//! Liveness and readiness for orchestrators: unauthenticated, tiny, and
//! outside `/api/v1`. The admin's site-health page is the detailed one.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde_json::json;

use crate::state::AppState;

/// `GET /healthz`: the process is up and bound. A restart cures what
/// this would report.
pub async fn healthz() -> impl IntoResponse {
    Json(json!({ "status": "ok" }))
}

/// `GET /readyz`: the database answers and no migration is pending.
/// Route traffic only when this is 200.
pub async fn readyz(State(state): State<AppState>) -> impl IntoResponse {
    let unreachable = || {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "status": "not-ready", "reason": "database unreachable" })),
        )
    };
    if sqlx::query("SELECT 1").execute(&state.pool).await.is_err() {
        return unreachable();
    }
    match vyasa_db::pending_migrations(&state.pool).await {
        Ok(pending) if pending.is_empty() => (StatusCode::OK, Json(json!({ "status": "ready" }))),
        Ok(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "status": "not-ready", "reason": "migrations pending" })),
        ),
        Err(_) => unreachable(),
    }
}
