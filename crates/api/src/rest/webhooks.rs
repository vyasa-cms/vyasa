//! Webhook CRUD + test-send (ManagePlugins capability).

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde_json::Value;

use vyasa_common::AppError;

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

fn repo(state: &AppState) -> vyasa_db::repo::WebhooksRepo {
    vyasa_db::repo::WebhooksRepo::new(state.pool.clone())
}

/// A fresh signing secret: 32 random bytes, hex, prefixed.
fn new_secret() -> String {
    use rand::RngCore as _;
    let mut buf = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut buf);
    format!("whsec_{}", hex::encode(buf))
}

/// Refuses event names the dispatcher never emits.
fn check_events(events: &Value) -> Result<(), ApiError> {
    if let Some(list) = events.as_array() {
        for entry in list {
            let name = entry.as_str().unwrap_or_default();
            if vyasa_core::webhooks::service::WebhookEvent::parse(name).is_none() {
                let known: Vec<&str> = vyasa_core::webhooks::service::WebhookEvent::ALL
                    .iter()
                    .map(|e| e.as_str())
                    .collect();
                return Err(ApiError(AppError::validation(format!(
                    "unknown event \"{name}\"; known events are {}",
                    known.join(", ")
                ))));
            }
        }
    }
    Ok(())
}

fn hook_json(r: &vyasa_db::repo::WebhookRow) -> Value {
    serde_json::json!({ "id": r.id, "url": r.url, "events": r.events, "enabled": r.enabled })
}

/// `POST /api/v1/webhooks` — register a webhook. Secret generated and
/// returned once.
#[utoipa::path(
    post, path = "/api/v1/webhooks", tag = "webhooks",
    security(("session_cookie" = [])),
    request_body(content = serde_json::Value),
    responses((status = 201, description = "Created"))
)]
pub async fn create(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let url = body["url"]
        .as_str()
        .ok_or_else(|| ApiError(AppError::validation("url required")))?;
    if !url.starts_with("https://") {
        return Err(ApiError(AppError::validation("url must be https")));
    }
    let events = body.get("events").cloned().unwrap_or(serde_json::json!([]));
    check_events(&events)?;
    let secret = new_secret();
    let row = repo(&state)
        .create(url, &secret, &events)
        .await
        .map_err(ApiError)?;
    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "id": row.id, "url": row.url, "events": row.events, "enabled": row.enabled,
            "secret": secret,
            "signature_header": vyasa_core::webhooks::SIGNATURE_HEADER,
        })),
    ))
}

/// `GET /api/v1/webhooks` — list webhooks (secret redacted).
#[utoipa::path(
    get, path = "/api/v1/webhooks", tag = "webhooks",
    security(("session_cookie" = [])),
    responses((status = 200, description = "Webhooks"))
)]
pub async fn list(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let r = repo(&state);
    let rows = r.list().await.map_err(ApiError)?;
    let last: std::collections::HashMap<i64, vyasa_db::repo::LastDelivery> = r
        .last_deliveries()
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|d| (d.webhook_id, d))
        .collect();
    let out: Vec<Value> = rows
        .iter()
        .map(|row| {
            let mut v = hook_json(row);
            v["last_delivery"] = match last.get(&row.id) {
                Some(d) => {
                    serde_json::json!({ "status": d.status, "at": d.last_attempt.to_rfc3339() })
                }
                None => Value::Null,
            };
            v
        })
        .collect();
    Ok(Json(Value::Array(out)))
}

/// `PATCH /api/v1/webhooks/{id}` — change the target, events or enabled
/// flag; anything omitted stays.
#[utoipa::path(
    patch, path = "/api/v1/webhooks/{id}", tag = "webhooks",
    security(("session_cookie" = [])),
    request_body(content = serde_json::Value),
    responses((status = 200, description = "Updated", body = serde_json::Value))
)]
pub async fn update(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(body): Json<Value>,
) -> ApiResult<Json<Value>> {
    let url = body.get("url").and_then(Value::as_str);
    if let Some(u) = url {
        if !u.starts_with("https://") {
            return Err(ApiError(AppError::validation("url must be https")));
        }
    }
    let events = body.get("events").cloned();
    if let Some(e) = &events {
        check_events(e)?;
    }
    let enabled = body.get("enabled").and_then(Value::as_bool);
    let row = repo(&state)
        .update(id, url, events.as_ref(), enabled)
        .await
        .map_err(ApiError)?;
    Ok(Json(hook_json(&row)))
}

/// `POST /api/v1/webhooks/{id}/rotate-secret` — a new signing secret,
/// returned once. Deliveries already queued keep the old signature.
#[utoipa::path(
    post, path = "/api/v1/webhooks/{id}/rotate-secret", tag = "webhooks",
    security(("session_cookie" = [])),
    responses((status = 200, description = "The new secret", body = serde_json::Value))
)]
pub async fn rotate_secret(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<Value>> {
    let secret = new_secret();
    repo(&state)
        .rotate_secret(id, &secret)
        .await
        .map_err(ApiError)?;
    Ok(Json(serde_json::json!({
        "id": id, "secret": secret,
        "signature_header": vyasa_core::webhooks::SIGNATURE_HEADER,
    })))
}

/// `DELETE /api/v1/webhooks/{id}` — delete webhook + deliveries.
#[utoipa::path(
    delete, path = "/api/v1/webhooks/{id}", tag = "webhooks",
    security(("session_cookie" = [])),
    responses((status = 204, description = "Deleted"))
)]
pub async fn delete(State(state): State<AppState>, Path(id): Path<i64>) -> ApiResult<StatusCode> {
    repo(&state).delete(id).await.map_err(ApiError)?;
    Ok(StatusCode::NO_CONTENT)
}

/// Queues one signed delivery for `hook`, exactly as the dispatcher does.
async fn enqueue_delivery(
    state: &AppState,
    hook: &vyasa_db::repo::WebhookRow,
    event: &str,
    body: String,
) -> Result<i64, ApiError> {
    let now = chrono::Utc::now().timestamp().unsigned_abs();
    let sig = vyasa_core::webhooks::service::sign(hook.secret.as_bytes(), now, &body);
    let payload = serde_json::json!({
        "webhook_id": hook.id, "event": event, "url": hook.url,
        "signature": sig, "body": body,
    });
    let id = vyasa_jobs::queue::enqueue(&state.pool, "webhook_deliver", payload, None)
        .await
        .map_err(ApiError)?;
    state.publisher_notify.notify_one();
    Ok(id)
}

/// `POST /api/v1/webhooks/{id}/test` — queue a signed `test.ping`.
///
/// It used to POST inline from the request handler, which blocked for up
/// to ten seconds and skipped the retry path real deliveries take. Now it
/// is a normal queued delivery; the outcome lands in the history.
#[utoipa::path(
    post, path = "/api/v1/webhooks/{id}/test", tag = "webhooks",
    security(("session_cookie" = [])),
    responses((status = 202, description = "Queued", body = serde_json::Value))
)]
pub async fn test_send(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let hook = repo(&state).get(id).await.map_err(ApiError)?;
    let now = chrono::Utc::now().timestamp().unsigned_abs();
    let body = vyasa_core::webhooks::service::envelope(
        "test.ping",
        now,
        serde_json::json!({ "webhook_id": id }),
    );
    let job = enqueue_delivery(&state, &hook, "test.ping", body).await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(serde_json::json!({ "queued": true, "job_id": job })),
    ))
}

/// `POST /api/v1/webhooks/{id}/deliveries/{delivery_id}/redeliver` — send
/// the same body again, freshly signed.
#[utoipa::path(
    post, path = "/api/v1/webhooks/{id}/deliveries/{delivery_id}/redeliver", tag = "webhooks",
    security(("session_cookie" = [])),
    responses((status = 202, description = "Queued", body = serde_json::Value),
              (status = 409, description = "That delivery has no stored body"))
)]
pub async fn redeliver(
    State(state): State<AppState>,
    Path((id, delivery_id)): Path<(i64, i64)>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let r = repo(&state);
    let hook = r.get(id).await.map_err(ApiError)?;
    let delivery = r.delivery(id, delivery_id).await.map_err(ApiError)?;
    let Some(body) = delivery.payload else {
        return Err(ApiError(AppError::conflict(
            "this delivery predates payload capture and cannot be resent",
        )));
    };
    let job = enqueue_delivery(&state, &hook, &delivery.event, body).await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(serde_json::json!({ "queued": true, "job_id": job })),
    ))
}

/// `GET /api/v1/webhooks/{id}/deliveries` — recent delivery attempts.
///
/// The rows have always been recorded; without this route the admin had no
/// way to tell a working webhook from a silently failing one.
#[utoipa::path(
    get, path = "/api/v1/webhooks/{id}/deliveries",
    tag = "webhooks",
    security(("session_cookie" = [])),
    params(("id" = i64, Path, description = "Webhook id")),
    responses((status = 200, description = "Delivery attempts", body = serde_json::Value))
)]
pub async fn deliveries(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<Value>> {
    // The route asks for the same capability as the rest of the page: it
    // used to demand ManageOptions, so a plugin manager saw a broken modal.
    let rows = repo(&state).deliveries(id, 50).await.map_err(ApiError)?;
    Ok(Json(serde_json::json!(rows
        .into_iter()
        .map(|d| serde_json::json!({
            "id": d.id,
            "event": d.event,
            "status": d.status,
            "response_code": d.response_code,
            "attempts": d.attempts,
            "at": d.last_attempt.to_rfc3339(),
            "payload": d.payload,
            "response_body": d.response_body,
            "error": d.error,
        }))
        .collect::<Vec<_>>())))
}
