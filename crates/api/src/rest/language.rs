//! Entry language + translation-group endpoints.

use axum::extract::{Path, State};
use axum::Json;
use serde::{Deserialize, Serialize};

use vyasa_common::AppError;
use vyasa_db::content_models::PostType;

use crate::error::{ApiError, ApiErrorBody, ApiResult};
use crate::middleware::Principal;
use crate::policy;
use crate::state::AppState;

/// What the editor sends.
#[derive(Deserialize, utoipa::ToSchema)]
pub struct SetLanguageRequest {
    /// BCP-47-ish tag (`en`, `pt-BR`); empty clears the declaration.
    pub lang: String,
    /// Slug of an entry of the same type this one translates (a page's
    /// slug among pages, a content type's among its entries); both join
    /// one group. The other entry must have declared its own language
    /// first. `translation_of` on `PUT /posts/{id}` links by id.
    #[serde(default)]
    pub link_slug: Option<String>,
}

/// One member of the translation group.
#[derive(Serialize, utoipa::ToSchema)]
pub struct TranslationMember {
    /// Entry id.
    pub id: i64,
    /// Its language.
    pub lang: String,
    /// Its title.
    pub title: String,
    /// Its slug.
    pub slug: String,
}

/// The entry's language story.
#[derive(Serialize, utoipa::ToSchema)]
pub struct LanguageResponse {
    /// Declared language, empty when none.
    pub lang: String,
    /// Published group members (self included when published).
    pub group: Vec<TranslationMember>,
}

/// `a-z` tag with optional subtags — permissive enough for real BCP-47,
/// strict enough that markup cannot ride in.
fn lang_ok(lang: &str) -> bool {
    let mut parts = lang.split('-');
    let Some(primary) = parts.next() else {
        return false;
    };
    (2..=3).contains(&primary.len())
        && primary.bytes().all(|b| b.is_ascii_lowercase())
        && parts.all(|p| (1..=8).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_alphanumeric()))
}

async fn response_for(
    state: &AppState,
    principal: &Principal,
    post_id: i64,
) -> Result<LanguageResponse, AppError> {
    let lang = state
        .translations
        .get(post_id)
        .await?
        .map(|t| t.lang)
        .unwrap_or_default();
    let group = if lang.is_empty() {
        Vec::new()
    } else {
        let mut members = Vec::new();
        for a in state.translations.alternates(post_id).await? {
            // A member of a type the caller may not read stays out.
            let readable = match vyasa_db::content_models::PostType::parse(&a.post_type) {
                Ok(t) => policy::may_read_type(state, Some(principal), t).await,
                Err(_) => false,
            };
            if readable {
                members.push(TranslationMember {
                    id: a.post_id,
                    lang: a.lang,
                    title: a.title,
                    slug: a.slug,
                });
            }
        }
        members
    };
    Ok(LanguageResponse { lang, group })
}

/// `GET /api/v1/posts/{id}/language`.
///
/// An entry's language is part of the entry: it is readable by whoever
/// may read the entry itself.
///
/// # Errors
/// 403 when the entry is not visible to the caller; 404 for an unknown
/// entry.
#[utoipa::path(
    get, path = "/api/v1/posts/{id}/language", tag = "posts",
    security(("session_cookie" = [])),
    params(("id" = i64, Path, description = "Post id")),
    responses(
        (status = 200, description = "Language + group", body = LanguageResponse),
        (status = 403, description = "Entry not visible", body = ApiErrorBody),
        (status = 404, description = "Unknown entry", body = ApiErrorBody),
    )
)]
pub async fn get(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<i64>,
) -> ApiResult<Json<LanguageResponse>> {
    policy::post_for_view(&state, &principal, id, None).await?;
    Ok(Json(response_for(&state, &principal, id).await?))
}

/// `PUT /api/v1/posts/{id}/language` — declare the language, optionally
/// joining another entry's translation group.
///
/// # Errors
/// 400 for a malformed tag, a `link_slug` that names no entry the caller
/// can see, or a target with no language of its own; 403 for someone
/// else's entry without `edit_others`; 404 for an unknown id.
#[utoipa::path(
    put, path = "/api/v1/posts/{id}/language", tag = "posts",
    security(("session_cookie" = [])),
    params(("id" = i64, Path, description = "Post id")),
    request_body = SetLanguageRequest,
    responses(
        (status = 200, description = "Language + group", body = LanguageResponse),
        (status = 400, description = "Validation failed", body = ApiErrorBody),
        (status = 403, description = "Missing edit_posts / edit_others", body = ApiErrorBody),
        (status = 404, description = "Unknown entry", body = ApiErrorBody),
    )
)]
pub async fn put(
    State(state): State<AppState>,
    principal: Principal,
    Path(id): Path<i64>,
    Json(body): Json<SetLanguageRequest>,
) -> ApiResult<Json<LanguageResponse>> {
    let entry = policy::post_for_edit(&state, &principal, id).await?;
    let lang = body.lang.trim();
    if lang.is_empty() {
        state.translations.clear(id).await?;
        return Ok(Json(response_for(&state, &principal, id).await?));
    }
    if !lang_ok(lang) {
        return Err(ApiError(AppError::validation(
            "lang must look like \"en\" or \"pt-BR\"",
        )));
    }
    // A link is for entries the caller can see. Everything about it is
    // decided before anything is written, so a refused or invalid link
    // leaves this entry as it was.
    let target = match body
        .link_slug
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(slug) => Some(link_target(&state, &principal, entry.post_type, slug).await?),
        None => None,
    };
    // Linking an entry to itself joins nothing.
    let group = match target.filter(|target| target.id != id) {
        Some(target) => {
            // A translation is of the same type as what it translates;
            // `link_target` only looks among the entry's own type.
            debug_assert_eq!(target.post_type, entry.post_type);
            let Some(theirs) = state.translations.get(target.id).await? else {
                return Err(ApiError(AppError::validation(format!(
                    "set a language on \"{}\" first, then link to it",
                    target.title
                ))));
            };
            Some(theirs.group_id)
        }
        None => None,
    };
    state.translations.set_lang(id, lang).await?;
    if let Some(group) = group {
        state.translations.join_group(id, group).await?;
    }
    // Cached entry pages carry hreflang now; a language edit must not
    // leave stale heads behind. Rare enough that a full purge is fine.
    if let Some(cache) = &state.render_cache {
        cache.invalidate_options();
    }
    Ok(Json(response_for(&state, &principal, id).await?))
}

/// The entry of `post_type` — the linking entry's own type: a
/// translation is of the same type — with `slug` that `principal` can see
/// without a read token. A page links to a page even where a post shares
/// its slug; an entry of a content type links to one of the same type
/// (as `translation_of` does on `PUT /posts/{id}`).
///
/// A slug that names only entries hidden from the caller is answered
/// exactly like one that names nothing, so the answer does not say that a
/// draft or private entry exists.
async fn link_target(
    state: &AppState,
    principal: &Principal,
    post_type: PostType,
    slug: &str,
) -> ApiResult<vyasa_db::content_models::PostRow> {
    match policy::post_for_view_by_slug(state, principal, post_type, slug, None).await {
        Err(ApiError(AppError::NotFound { .. } | AppError::Forbidden { .. })) => Err(ApiError(
            AppError::validation(format!("no {} has the slug {slug:?}", post_type.as_str())),
        )),
        decided => decided,
    }
}

#[cfg(test)]
mod tests {
    use super::lang_ok;

    #[test]
    fn language_tags_are_shaped_not_trusted() {
        assert!(lang_ok("en"));
        assert!(lang_ok("pt-BR"));
        assert!(lang_ok("zh-Hant"));
        assert!(!lang_ok(""));
        assert!(!lang_ok("e"));
        assert!(!lang_ok("english-language-tag"));
        assert!(!lang_ok("en\"><script"));
    }
}
