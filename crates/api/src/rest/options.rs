//! Site options endpoints: authenticated read/write (ManageOptions) and
//! the public subset.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde_json::{Map, Value};

use vyasa_core::user::Capability;

use crate::error::{ApiErrorBody, ApiResult};
use crate::middleware::Principal;
use crate::state::AppState;

/// `GET /api/v1/options` — all managed options. Non-public keys require
/// `ManageOptions`; with the cap you get everything, without it only the
/// public subset.
pub async fn get_all(
    State(state): State<AppState>,
    principal: Principal,
) -> ApiResult<Json<Map<String, Value>>> {
    // Read everything when allowed; otherwise only the public subset.
    let is_manager = principal.ensure(Capability::ManageOptions).is_ok();
    let keys: &[&str] = if is_manager {
        vyasa_core::options::SITE_OPTION_KEYS
    } else {
        vyasa_core::options::PUBLIC_OPTION_KEYS
    };
    let mut out = Map::new();
    for key in keys {
        if let Ok(v) = state.options.get(key).await {
            out.insert((*key).to_owned(), v);
        }
    }
    Ok(Json(out))
}

/// `PUT /api/v1/options/{key}` — write one option (ManageOptions).
///
/// An option that redirects mail, links, updates or package trust takes a
/// full administrator (`policy::option_write`).
///
/// Validation runs per key (permalink grammar, numeric ranges, enum values,
/// custom-CSS sanitization); a successful write emits `OptionChanged` so
/// render caches purge.
pub async fn put(
    State(state): State<AppState>,
    principal: Principal,
    Path(key): Path<String>,
    Json(value): Json<Value>,
) -> ApiResult<StatusCode> {
    crate::policy::demo_options(&state, [key.as_str()])?;
    crate::policy::option_write(&principal, [key.as_str()])?;
    state.options_service.put(&key, value).await?;
    // Site settings can appear anywhere: drop every rendered page.
    if let Some(cache) = &state.render_cache {
        cache.invalidate_options();
    }
    // The value itself is not recorded: custom_css and site identity can be
    // large, and the useful question is who changed what and when.
    crate::audit::record(
        &state,
        principal.user(),
        "options.write",
        format!("option:{key}"),
        serde_json::json!({}),
    );
    // The key, never the value: an option may hold a secret, and a plugin
    // that wants one has to ask for `db:read:options` and be granted it.
    crate::plugin_hooks::emit(
        &state,
        crate::plugin_hooks::events::OPTION_CHANGED,
        serde_json::json!({ "key": key }),
    );
    Ok(StatusCode::NO_CONTENT)
}

/// `PUT /api/v1/options` — write several options in one request.
///
/// The admin's settings page saves a whole form. Doing that as one PUT per
/// key means a failure halfway leaves the site half-updated, so this
/// validates every key first and only then writes: either all of them land
/// or none do.
///
/// Validation is the same per-key validation `put` performs; running it up
/// front is what makes the write all-or-nothing. So is the permission: one
/// option that takes a full administrator (`policy::option_write`) refuses
/// the whole request for a caller who is not one.
#[utoipa::path(
    put, path = "/api/v1/options",
    tag = "options",
    security(("session_cookie" = [])),
    request_body = serde_json::Value,
    responses(
        (status = 204, description = "All options written"),
        (status = 400, description = "One or more values rejected; nothing written", body = ApiErrorBody),
        (status = 403, description = "Forbidden", body = ApiErrorBody),
    )
)]
pub async fn put_many(
    State(state): State<AppState>,
    principal: Principal,
    Json(body): Json<Map<String, Value>>,
) -> ApiResult<StatusCode> {
    crate::policy::demo_options(&state, body.keys().map(String::as_str))?;
    crate::policy::option_write(&principal, body.keys().map(String::as_str))?;
    // Validate the whole batch before touching storage.
    for (key, value) in &body {
        state
            .options_service
            .validate_with_lookups(key, value)
            .await?;
    }

    let keys: Vec<String> = body.keys().cloned().collect();
    for (key, value) in body {
        state.options_service.put(&key, value).await?;
    }

    if let Some(cache) = &state.render_cache {
        cache.invalidate_options();
    }
    crate::audit::record(
        &state,
        principal.user(),
        "options.write",
        format!("options:{}", keys.len()),
        serde_json::json!({ "keys": keys }),
    );
    for key in &keys {
        crate::plugin_hooks::emit(
            &state,
            crate::plugin_hooks::events::OPTION_CHANGED,
            serde_json::json!({ "key": key }),
        );
    }
    Ok(StatusCode::NO_CONTENT)
}
