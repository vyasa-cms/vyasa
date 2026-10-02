//! `/api/v1/forms`: definitions and their submissions.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::error::ApiResult;
use crate::forms::{self, FormInput, FormRow};
use crate::state::AppState;

/// A form with its unread count.
#[derive(Serialize, utoipa::ToSchema)]
pub struct FormWithCount {
    #[serde(flatten)]
    pub form: FormRow,
    pub unread: i64,
}

/// `GET /api/v1/forms`
#[utoipa::path(get, path = "/api/v1/forms", tag = "content",
    responses((status = 200, description = "Every form", body = [FormWithCount])))]
pub async fn list(State(state): State<AppState>) -> ApiResult<Json<Vec<FormWithCount>>> {
    let unread = state.audience.unread_by_form().await.unwrap_or_default();
    Ok(Json(
        forms::list(&state)
            .await?
            .into_iter()
            .map(|f| {
                let n = unread
                    .iter()
                    .find(|(s, _)| s == &f.slug)
                    .map_or(0, |(_, n)| *n);
                FormWithCount { form: f, unread: n }
            })
            .collect(),
    ))
}

/// `POST /api/v1/forms`
#[utoipa::path(post, path = "/api/v1/forms", tag = "content", request_body = FormInput,
    responses((status = 201, description = "Created", body = FormRow)))]
pub async fn create(
    State(state): State<AppState>,
    Json(input): Json<FormInput>,
) -> ApiResult<(StatusCode, Json<FormRow>)> {
    Ok((
        StatusCode::CREATED,
        Json(forms::create(&state, input).await?),
    ))
}

/// `PUT /api/v1/forms/{id}`
#[utoipa::path(put, path = "/api/v1/forms/{id}", tag = "content", request_body = FormInput,
    responses((status = 200, description = "Updated", body = FormRow)))]
pub async fn update(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(input): Json<FormInput>,
) -> ApiResult<Json<FormRow>> {
    let row = forms::update(&state, id, input).await?;
    state
        .surface_epoch
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    Ok(Json(row))
}

/// `DELETE /api/v1/forms/{id}` — the definition; submissions stay.
#[utoipa::path(delete, path = "/api/v1/forms/{id}", tag = "content",
    responses((status = 204, description = "Removed")))]
pub async fn remove(State(state): State<AppState>, Path(id): Path<i64>) -> ApiResult<StatusCode> {
    forms::remove(&state, id).await?;
    state
        .surface_epoch
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    Ok(StatusCode::NO_CONTENT)
}

/// One answer as the inbox shows it.
#[derive(Serialize, utoipa::ToSchema)]
pub struct Submission {
    pub id: i64,
    pub name: String,
    pub email: String,
    pub message: String,
    pub path: String,
    pub data: serde_json::Value,
    pub read_at: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// `GET /api/v1/forms/{id}/submissions`
#[utoipa::path(get, path = "/api/v1/forms/{id}/submissions", tag = "content",
    responses((status = 200, description = "Newest first", body = [Submission])))]
pub async fn submissions(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<Vec<Submission>>> {
    let form = forms::get(&state, id).await?;
    let rows = state.audience.submissions_for(&form.slug, 500).await?;
    Ok(Json(
        rows.into_iter()
            .map(|r| Submission {
                id: r.id,
                name: r.name,
                email: r.email,
                message: r.message,
                path: r.path,
                data: r.data,
                read_at: r.read_at,
                created_at: r.created_at,
            })
            .collect(),
    ))
}

/// `GET /api/v1/forms/{id}/submissions.csv`
#[utoipa::path(get, path = "/api/v1/forms/{id}/submissions.csv", tag = "content",
    responses((status = 200, description = "A spreadsheet")))]
pub async fn submissions_csv(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<(axum::http::HeaderMap, String)> {
    let form = forms::get(&state, id).await?;
    let fields: Vec<forms::Field> = serde_json::from_value(form.fields.clone()).unwrap_or_default();
    let rows = state.audience.submissions_for(&form.slug, 10_000).await?;
    let mut out = String::new();
    out.push_str("received,page,");
    out.push_str(
        &fields
            .iter()
            .map(|f| cell(&f.label))
            .collect::<Vec<_>>()
            .join(","),
    );
    out.push('\n');
    for r in rows {
        out.push_str(&cell(&r.created_at.to_rfc3339()));
        out.push(',');
        out.push_str(&cell(&r.path));
        for f in &fields {
            out.push(',');
            out.push_str(&cell(
                r.data.get(&f.key).and_then(|v| v.as_str()).unwrap_or(""),
            ));
        }
        out.push('\n');
    }
    let mut h = axum::http::HeaderMap::new();
    h.insert(
        axum::http::header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static("text/csv; charset=utf-8"),
    );
    if let Ok(v) = axum::http::HeaderValue::from_str(&format!(
        "attachment; filename=\"{}-submissions.csv\"",
        form.slug
    )) {
        h.insert(axum::http::header::CONTENT_DISPOSITION, v);
    }
    Ok((h, out))
}

/// One CSV cell: quoted, with quotes doubled, and — because a visitor
/// wrote most of these values — neutralised against formula injection: a
/// cell a spreadsheet would evaluate (`=`, `+`, `-`, `@`, tab, CR) gets a
/// leading `'` so it opens as text.
fn cell(s: &str) -> String {
    let guard = if s.starts_with(['=', '+', '-', '@', '\t', '\r']) {
        "'"
    } else {
        ""
    };
    format!("\"{guard}{}\"", s.replace('"', "\"\""))
}

/// Ids to mark read.
#[derive(Deserialize, utoipa::ToSchema)]
pub struct ReadBody {
    pub ids: Vec<i64>,
}

/// `POST /api/v1/forms/submissions/read`
#[utoipa::path(post, path = "/api/v1/forms/submissions/read", tag = "content", request_body = ReadBody,
    responses((status = 200, description = "How many changed", body = serde_json::Value)))]
pub async fn mark_read(
    State(state): State<AppState>,
    Json(body): Json<ReadBody>,
) -> ApiResult<Json<serde_json::Value>> {
    let n = state.audience.mark_read(&body.ids).await?;
    Ok(Json(serde_json::json!({ "read": n })))
}

#[cfg(test)]
mod tests {
    use super::cell;

    #[test]
    fn csv_cells_that_a_spreadsheet_would_evaluate_open_as_text() {
        assert_eq!(cell("=HYPERLINK(\"x\")"), "\"'=HYPERLINK(\"\"x\"\")\"");
        for bad in ["+1", "-2+3", "@SUM(A1)", "\tx", "\rx"] {
            assert!(cell(bad).starts_with("\"'"), "{bad}");
        }
        assert_eq!(cell("hello"), "\"hello\"");
        assert_eq!(cell(""), "\"\"");
    }
}
