//! Saved arrangements of blocks. An unsynced pattern is copied into a
//! document on insert and forgotten; a synced one stays a reference
//! (`BlockKind::Pattern` with `attrs.pattern_id`) and is resolved into
//! its current blocks every time the page renders.

use serde::{Deserialize, Serialize};
use vyasa_common::AppError;
use vyasa_core::block::{Block, BlockKind};

use crate::state::AppState;

/// One saved pattern.
#[derive(Serialize, Deserialize, sqlx::FromRow, utoipa::ToSchema, Clone)]
pub struct PatternRow {
    pub id: i64,
    pub name: String,
    pub slug: String,
    pub category: String,
    pub synced: bool,
    /// The blocks, as a JSON array.
    pub blocks: serde_json::Value,
    pub created_by: Option<i64>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// What create and update take.
#[derive(Deserialize, utoipa::ToSchema)]
pub struct PatternInput {
    pub name: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub synced: bool,
    /// A JSON array of blocks.
    pub blocks: serde_json::Value,
}

const COLS: &str = "id, name, slug, category, synced, blocks, created_by, created_at, updated_at";
/// Synced patterns per page; a pattern that includes itself stops here.
const MAX_RESOLVED: usize = 64;

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
        String::from("pattern")
    } else {
        base
    }
}

fn check(input: &PatternInput) -> Result<(), AppError> {
    if input.name.trim().is_empty() {
        return Err(AppError::validation("a pattern needs a name"));
    }
    let Some(list) = input.blocks.as_array() else {
        return Err(AppError::validation("blocks must be a JSON array"));
    };
    if list.is_empty() {
        return Err(AppError::validation("a pattern needs at least one block"));
    }
    for b in list {
        let block: Block = serde_json::from_value(b.clone())
            .map_err(|e| AppError::validation(format!("block: {e}")))?;
        if block.kind == BlockKind::Pattern && input.synced {
            return Err(AppError::validation(
                "a synced pattern cannot contain another synced pattern",
            ));
        }
    }
    Ok(())
}

/// # Errors
/// Database errors.
pub async fn list(state: &AppState) -> Result<Vec<PatternRow>, AppError> {
    sqlx::query_as::<_, PatternRow>(sqlx::AssertSqlSafe(format!(
        "SELECT {COLS} FROM patterns ORDER BY category, name"
    )))
    .fetch_all(&state.pool)
    .await
    .map_err(|e| AppError::db(format!("patterns: {e}")))
}

/// # Errors
/// [`AppError::NotFound`] when missing.
pub async fn get(state: &AppState, id: i64) -> Result<PatternRow, AppError> {
    sqlx::query_as::<_, PatternRow>(sqlx::AssertSqlSafe(format!(
        "SELECT {COLS} FROM patterns WHERE id = $1"
    )))
    .bind(id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| AppError::db(format!("pattern: {e}")))?
    .ok_or_else(|| AppError::not_found("pattern", id))
}

/// # Errors
/// Validation; a duplicate name; database errors.
pub async fn create(
    state: &AppState,
    by: i64,
    input: PatternInput,
) -> Result<PatternRow, AppError> {
    check(&input)?;
    let mut slug = slugify(&input.name);
    // A second "Call to action" becomes call-to-action-2, not a conflict.
    for n in 2..100 {
        let taken: bool =
            sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM patterns WHERE slug = $1)")
                .bind(&slug)
                .fetch_one(&state.pool)
                .await
                .map_err(|e| AppError::db(format!("pattern slug: {e}")))?;
        if !taken {
            break;
        }
        slug = format!("{}-{n}", slugify(&input.name));
    }
    sqlx::query_as::<_, PatternRow>(sqlx::AssertSqlSafe(format!(
        "INSERT INTO patterns (id, name, slug, category, synced, blocks, created_by)
         VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING {COLS}"
    )))
    .bind(vyasa_common::next_id_i64())
    .bind(input.name.trim())
    .bind(&slug)
    .bind(input.category.trim())
    .bind(input.synced)
    .bind(&input.blocks)
    .bind(by)
    .fetch_one(&state.pool)
    .await
    .map_err(|e| AppError::db(format!("pattern insert: {e}")))
}

/// # Errors
/// Validation; not found; database errors.
pub async fn update(
    state: &AppState,
    id: i64,
    input: PatternInput,
) -> Result<PatternRow, AppError> {
    check(&input)?;
    sqlx::query_as::<_, PatternRow>(sqlx::AssertSqlSafe(format!(
        "UPDATE patterns SET name = $2, category = $3, synced = $4, blocks = $5, updated_at = now()
         WHERE id = $1 RETURNING {COLS}"
    )))
    .bind(id)
    .bind(input.name.trim())
    .bind(input.category.trim())
    .bind(input.synced)
    .bind(&input.blocks)
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| AppError::db(format!("pattern update: {e}")))?
    .ok_or_else(|| AppError::not_found("pattern", id))
}

/// # Errors
/// Not found; database errors.
pub async fn remove(state: &AppState, id: i64) -> Result<(), AppError> {
    let n = sqlx::query("DELETE FROM patterns WHERE id = $1")
        .bind(id)
        .execute(&state.pool)
        .await
        .map_err(|e| AppError::db(format!("pattern delete: {e}")))?
        .rows_affected();
    if n == 0 {
        return Err(AppError::not_found("pattern", id));
    }
    Ok(())
}

fn has_patterns(blocks: &[Block]) -> bool {
    blocks
        .iter()
        .any(|b| b.kind == BlockKind::Pattern || has_patterns(&b.children))
}

/// Fills every synced pattern block's children from the saved pattern,
/// so the renderer sees ordinary blocks. Bounded, and a pattern cannot
/// nest another synced pattern, so this cannot loop.
pub async fn resolve(state: &AppState, blocks: &mut [Block]) {
    if !has_patterns(blocks) {
        return;
    }
    let mut budget = MAX_RESOLVED;
    resolve_list(state, blocks, &mut budget).await;
}

fn resolve_list<'a>(
    state: &'a AppState,
    blocks: &'a mut [Block],
    budget: &'a mut usize,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
    Box::pin(async move {
        for block in blocks.iter_mut() {
            if block.kind == BlockKind::Pattern {
                if *budget == 0 {
                    block.children.clear();
                    continue;
                }
                *budget -= 1;
                let id = block.attrs.get("pattern_id").and_then(|v| {
                    v.as_str()
                        .and_then(|s| s.parse::<i64>().ok())
                        .or_else(|| v.as_i64())
                });
                let resolved = match id {
                    Some(id) => get(state, id).await.ok(),
                    None => None,
                };
                block.children = resolved
                    .and_then(|p| serde_json::from_value::<Vec<Block>>(p.blocks).ok())
                    .unwrap_or_default();
                // A pattern's own children may hold plugin blocks or
                // unsynced content; nothing inside can be another synced
                // pattern, so no further resolution is needed here.
                continue;
            }
            if !block.children.is_empty() {
                resolve_list(state, &mut block.children, budget).await;
            }
        }
    })
}
