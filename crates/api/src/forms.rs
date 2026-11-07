//! Forms an editor designs: a definition with fields, rendered by the
//! `form` block, posted to the same endpoint as the fixed contact form,
//! answered into `form_submissions.data`, and mailed to whoever the form
//! names.

use rand::Rng;
use serde::{Deserialize, Serialize};
use vyasa_common::AppError;
use vyasa_core::block::{Block, BlockKind};

use crate::state::AppState;

/// One field of a form.
#[derive(Serialize, Deserialize, Clone, utoipa::ToSchema)]
pub struct Field {
    /// The input name; letters, digits, dashes, underscores.
    pub key: String,
    pub label: String,
    /// `text`, `email`, `textarea`, `number`, `select`, `checkbox`.
    #[serde(default = "text")]
    pub kind: String,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub options: Vec<String>,
}
fn text() -> String {
    String::from("text")
}

/// A form as stored and returned.
#[derive(Serialize, Deserialize, sqlx::FromRow, utoipa::ToSchema, Clone)]
pub struct FormRow {
    pub id: i64,
    pub name: String,
    pub slug: String,
    /// `Field`s as JSON.
    pub fields: serde_json::Value,
    pub notify_email: String,
    pub success_message: String,
    pub enabled: bool,
    /// Whether a submission waits for the submitter's email confirmation.
    pub confirm: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// What create and update take.
#[derive(Deserialize, utoipa::ToSchema)]
pub struct FormInput {
    pub name: String,
    pub fields: Vec<Field>,
    #[serde(default)]
    pub notify_email: String,
    #[serde(default)]
    pub success_message: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Hold a submission until its sender confirms by email (needs an email field).
    #[serde(default)]
    pub confirm: bool,
}
fn yes() -> bool {
    true
}

pub(crate) use vyasa_db::repo::audience::PENDING_FORM_DAYS as PENDING_DAYS;

const COLS: &str =
    "id, name, slug, fields, notify_email, success_message, enabled, confirm, created_at, updated_at";

fn slugify(name: &str) -> String {
    let base: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if base.is_empty() {
        String::from("form")
    } else {
        base
    }
}

fn check(input: &FormInput) -> Result<(), AppError> {
    if input.name.trim().is_empty() {
        return Err(AppError::validation("a form needs a name"));
    }
    if input.fields.is_empty() || input.fields.len() > 40 {
        return Err(AppError::validation("a form needs between 1 and 40 fields"));
    }
    let mut seen = std::collections::HashSet::new();
    for f in &input.fields {
        if f.key.is_empty()
            || f.key.len() > 40
            || !f
                .key
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
            || f.key == "website"
            || f.key == "form"
        {
            return Err(AppError::validation(format!(
                "field key {:?} must be letters, digits, dashes or underscores (and not \"website\" or \"form\")",
                f.key
            )));
        }
        if !seen.insert(f.key.as_str()) {
            return Err(AppError::validation(format!(
                "field key {:?} is used twice",
                f.key
            )));
        }
        if !matches!(
            f.kind.as_str(),
            "text" | "email" | "textarea" | "number" | "select" | "checkbox"
        ) {
            return Err(AppError::validation(format!(
                "unknown field kind {:?}",
                f.kind
            )));
        }
        if f.kind == "select" && f.options.is_empty() {
            return Err(AppError::validation(format!(
                "select field {:?} needs options",
                f.key
            )));
        }
    }
    if !input.notify_email.trim().is_empty() && !input.notify_email.contains('@') {
        return Err(AppError::validation(
            "the notification address is not an email",
        ));
    }
    if input.confirm && !input.fields.iter().any(|f| f.kind == "email" && f.required) {
        return Err(AppError::validation(
            "a form that asks for confirmation needs a required email field",
        ));
    }
    Ok(())
}

/// # Errors
/// Database errors.
pub async fn list(state: &AppState) -> Result<Vec<FormRow>, AppError> {
    sqlx::query_as::<_, FormRow>(sqlx::AssertSqlSafe(format!(
        "SELECT {COLS} FROM forms ORDER BY name"
    )))
    .fetch_all(&state.pool)
    .await
    .map_err(|e| AppError::db(format!("forms: {e}")))
}

/// # Errors
/// [`AppError::NotFound`] when missing.
pub async fn get(state: &AppState, id: i64) -> Result<FormRow, AppError> {
    sqlx::query_as::<_, FormRow>(sqlx::AssertSqlSafe(format!(
        "SELECT {COLS} FROM forms WHERE id = $1"
    )))
    .bind(id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| AppError::db(format!("form: {e}")))?
    .ok_or_else(|| AppError::not_found("form", id))
}

/// The enabled form behind a slug, if any.
pub async fn by_slug(state: &AppState, slug: &str) -> Option<FormRow> {
    sqlx::query_as::<_, FormRow>(sqlx::AssertSqlSafe(format!(
        "SELECT {COLS} FROM forms WHERE slug = $1 AND enabled"
    )))
    .bind(slug)
    .fetch_optional(&state.pool)
    .await
    .ok()
    .flatten()
}

/// # Errors
/// Validation; database errors.
pub async fn create(state: &AppState, input: FormInput) -> Result<FormRow, AppError> {
    check(&input)?;
    let mut slug = slugify(&input.name);
    for n in 2..100 {
        let taken: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM forms WHERE slug = $1)")
            .bind(&slug)
            .fetch_one(&state.pool)
            .await
            .map_err(|e| AppError::db(format!("form slug: {e}")))?;
        if !taken {
            break;
        }
        slug = format!("{}-{n}", slugify(&input.name));
    }
    let success = if input.success_message.trim().is_empty() {
        "Thanks — your message has been received.".to_owned()
    } else {
        input.success_message.trim().to_owned()
    };
    sqlx::query_as::<_, FormRow>(sqlx::AssertSqlSafe(format!(
        "INSERT INTO forms (id, name, slug, fields, notify_email, success_message, enabled, confirm)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8) RETURNING {COLS}"
    )))
    .bind(vyasa_common::next_id_i64())
    .bind(input.name.trim())
    .bind(&slug)
    .bind(serde_json::json!(input.fields))
    .bind(input.notify_email.trim())
    .bind(success)
    .bind(input.enabled)
    .bind(input.confirm)
    .fetch_one(&state.pool)
    .await
    .map_err(|e| AppError::db(format!("form insert: {e}")))
}

/// # Errors
/// Validation; not found; database errors.
pub async fn update(state: &AppState, id: i64, input: FormInput) -> Result<FormRow, AppError> {
    check(&input)?;
    let success = if input.success_message.trim().is_empty() {
        "Thanks — your message has been received.".to_owned()
    } else {
        input.success_message.trim().to_owned()
    };
    sqlx::query_as::<_, FormRow>(sqlx::AssertSqlSafe(format!(
        "UPDATE forms SET name = $2, fields = $3, notify_email = $4, success_message = $5,
                enabled = $6, confirm = $7, updated_at = now() WHERE id = $1 RETURNING {COLS}"
    )))
    .bind(id)
    .bind(input.name.trim())
    .bind(serde_json::json!(input.fields))
    .bind(input.notify_email.trim())
    .bind(success)
    .bind(input.enabled)
    .bind(input.confirm)
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| AppError::db(format!("form update: {e}")))?
    .ok_or_else(|| AppError::not_found("form", id))
}

/// # Errors
/// Not found; database errors.
pub async fn remove(state: &AppState, id: i64) -> Result<(), AppError> {
    let n = sqlx::query("DELETE FROM forms WHERE id = $1")
        .bind(id)
        .execute(&state.pool)
        .await
        .map_err(|e| AppError::db(format!("form delete: {e}")))?
        .rows_affected();
    if n == 0 {
        return Err(AppError::not_found("form", id));
    }
    Ok(())
}

fn has_forms(blocks: &[Block]) -> bool {
    blocks
        .iter()
        .any(|b| b.kind == BlockKind::Form || has_forms(&b.children))
}

/// Puts each form block's definition on its attrs so the renderer can
/// build the markup. A disabled or missing form renders nothing.
pub async fn resolve(state: &AppState, blocks: &mut [Block]) {
    if !has_forms(blocks) {
        return;
    }
    resolve_list(state, blocks).await;
}

fn resolve_list<'a>(
    state: &'a AppState,
    blocks: &'a mut [Block],
) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
    Box::pin(async move {
        for block in blocks.iter_mut() {
            if block.kind == BlockKind::Form {
                let slug = block
                    .attrs
                    .get("form_slug")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_owned();
                let def = by_slug(state, &slug).await.map(
                    |f| serde_json::json!({ "slug": f.slug, "name": f.name, "fields": f.fields }),
                );
                if let Some(obj) = block.attrs.as_object_mut() {
                    match def {
                        Some(d) => {
                            obj.insert("definition".into(), d);
                        }
                        None => {
                            obj.remove("definition");
                        }
                    }
                }
            }
            if !block.children.is_empty() {
                resolve_list(state, &mut block.children).await;
            }
        }
    })
}

/// What the public endpoint does with a defined form's answers: checks
/// required fields, stores them, mails the notification. Returns the
/// success message, or the validation problem.
///
/// # Errors
/// [`AppError::Validation`] with the first problem.
pub async fn accept(
    state: &AppState,
    form: &FormRow,
    values: &[(String, String)],
    path: &str,
) -> Result<String, AppError> {
    let fields: Vec<Field> = serde_json::from_value(form.fields.clone()).unwrap_or_default();
    let get = |key: &str| {
        values
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.trim().to_owned())
            .unwrap_or_default()
    };
    let mut data = serde_json::Map::new();
    let mut lines = Vec::new();
    for f in &fields {
        let raw = get(&f.key);
        let value: String = raw
            .chars()
            .take(if f.kind == "textarea" { 4000 } else { 300 })
            .collect();
        if f.required && value.is_empty() {
            return Err(AppError::validation(format!("{} is required", f.label)));
        }
        if f.kind == "email" && !value.is_empty() && !value.contains('@') {
            return Err(AppError::validation(format!(
                "{} must be an email address",
                f.label
            )));
        }
        if f.kind == "select" && !value.is_empty() && !f.options.iter().any(|o| o == &value) {
            return Err(AppError::validation(format!(
                "{} must be one of the offered choices",
                f.label
            )));
        }
        lines.push(serde_json::json!({ "label": f.label, "value": if f.kind == "checkbox" { if value.is_empty() { "no" } else { "yes" }.to_owned() } else { value.clone() } }));
        data.insert(f.key.clone(), serde_json::Value::String(value));
    }
    // Name, email and message columns get the obvious fields, so the
    // Audience inbox keeps showing something useful.
    let pick = |kinds: &[&str], keys: &[&str]| {
        fields
            .iter()
            .find(|f| keys.contains(&f.key.as_str()) || kinds.contains(&f.kind.as_str()))
            .map(|f| get(&f.key))
            .unwrap_or_default()
    };
    let name = pick(&[], &["name", "full_name", "your_name"]);
    let email = pick(&["email"], &["email"]);
    let message = pick(&["textarea"], &["message"]);
    let data = serde_json::Value::Object(data);
    if form.confirm {
        // The address the confirmation goes to is a required email field, so
        // an optional one left empty cannot skip the confirmation.
        let to = fields
            .iter()
            .find(|f| f.kind == "email" && f.required)
            .map(|f| get(&f.key))
            .unwrap_or_default();
        if !crate::public::form_submit::email_ok(&to) {
            return Err(AppError::validation(
                "that email address doesn't look right",
            ));
        }
        return hold_entry(state, form, &to, &name, &message, path, &data).await;
    }
    state
        .audience
        .add_submission_with(&form.slug, &name, &email, &message, path, &data)
        .await?;
    notify_owner(state, form, &lines, path, true).await;
    Ok(form.success_message.clone())
}

/// Stores an entry as held and mails its sender the confirm and delete links.
async fn hold_entry(
    state: &AppState,
    form: &FormRow,
    to: &str,
    name: &str,
    message: &str,
    path: &str,
    data: &serde_json::Value,
) -> Result<String, AppError> {
    let base = state
        .options_service
        .site_url()
        .await
        .map_err(|e| AppError::internal(e, "the site URL is not configured"))?;
    if base.is_empty() {
        return Err(AppError::validation(
            "this form cannot send confirmations: the site URL is not set",
        ));
    }
    let token: String = rand::thread_rng()
        .sample_iter(&rand::distributions::Alphanumeric)
        .take(40)
        .map(char::from)
        .collect();
    // A repeat submission from the same address replaces its held entry.
    state.audience.drop_pending_for(&form.slug, to).await?;
    state
        .audience
        .add_pending_submission(&vyasa_db::repo::audience::PendingSubmission {
            form: &form.slug,
            name,
            email: to,
            message,
            path,
            data,
            token: &token,
        })
        .await?;
    let base = base.trim_end_matches('/').to_owned();
    let ctx = serde_json::json!({
        "site": site_title(state).await,
        "form": form.name,
        "confirm_url": format!("{base}/form/confirm?token={token}"),
        "delete_url": format!("{base}/form/delete?token={token}"),
    });
    if let Err(e) = vyasa_core::notify::EmailService::new(state.pool.clone())
        .queue(to, "form_confirm", &ctx)
        .await
    {
        // No mail went out, so nobody can ever confirm this row.
        let _ = state.audience.delete_by_token(&token).await;
        return Err(AppError::internal(e, "confirmation not queued"));
    }
    state.publisher_notify.notify_one();
    Ok(String::from("Check your inbox to confirm your entry."))
}

async fn site_title(state: &AppState) -> String {
    state
        .options_service
        .site_identity()
        .await
        .ok()
        .map(|i| i.title)
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| String::from("Vyasa"))
}

/// Mails the form's `notify_email` the answers of one accepted entry.
/// With `include_answers` false (a confirmed entry) the mail names the form
/// only: the answers, and the sender's address, stay in the admin.
pub async fn notify_owner(
    state: &AppState,
    form: &FormRow,
    lines: &[serde_json::Value],
    path: &str,
    include_answers: bool,
) {
    if form.notify_email.is_empty() {
        return;
    }
    let ctx = serde_json::json!({
        "site": site_title(state).await, "form": form.name, "fields": lines, "path": path,
    });
    let template = if include_answers {
        "form_submission"
    } else {
        "form_confirmed_notice"
    };
    if let Err(e) = vyasa_core::notify::EmailService::new(state.pool.clone())
        .queue(&form.notify_email, template, &ctx)
        .await
    {
        tracing::warn!("form notification not queued: {e}");
    } else {
        state.publisher_notify.notify_one();
    }
}
