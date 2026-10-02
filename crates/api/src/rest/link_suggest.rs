//! Internal link suggestions: the site reads a draft and points out
//! where it already talks about things the site has pages for.
//!
//! No model, no external data — precision comes from the strictest
//! possible signal: the draft literally contains another entry's title.
//! Embedding neighbours pad the list as "related reading" when the
//! draft has an id, so even a title-less match set is useful. What the
//! author does with a suggestion stays in the editor; this endpoint
//! never writes.

use axum::extract::State;
use axum::Json;
use serde::{Deserialize, Serialize};

use vyasa_db::content_models::PostStatus;
use vyasa_db::repo::PostFilter;

use crate::error::{ApiErrorBody, ApiResult};
use crate::middleware::Principal;
use crate::state::AppState;

/// What the editor sends: the draft as it stands, unsaved included.
#[derive(Deserialize, utoipa::ToSchema)]
pub struct LinkSuggestRequest {
    /// The block document being written.
    pub content: serde_json::Value,
    /// The draft's own id, so it never suggests linking to itself (and
    /// unlocks embedding neighbours).
    #[serde(default, deserialize_with = "crate::flexible_id::option")]
    pub exclude_post_id: Option<i64>,
}

/// One suggestion.
#[derive(Serialize, utoipa::ToSchema)]
pub struct LinkSuggestion {
    /// The exact text in the draft to wrap, when a title matched;
    /// absent for a related-reading suggestion.
    pub phrase: Option<String>,
    /// Target entry id.
    #[serde(serialize_with = "id_string")]
    pub post_id: i64,
    /// Target title.
    pub title: String,
    /// Site-relative URL to link to.
    pub url: String,
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn id_string<S: serde::Serializer>(id: &i64, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&id.to_string())
}

/// Everything the matcher needs to know about one published entry.
struct Candidate {
    id: i64,
    title: String,
    url: String,
}

/// Title-in-draft matches, most specific (longest title) first.
///
/// A short title like "Home" would match half the drafts on the site,
/// so only titles of eight characters or more qualify; an entry the
/// draft already links to is never re-suggested.
fn phrase_matches<'a>(
    text_lower: &str,
    existing_hrefs: &[String],
    candidates: &'a [Candidate],
) -> Vec<&'a Candidate> {
    let mut hits: Vec<&Candidate> = candidates
        .iter()
        .filter(|c| c.title.chars().count() >= 8)
        .filter(|c| text_lower.contains(&c.title.to_lowercase()))
        .filter(|c| !existing_hrefs.iter().any(|h| h == &c.url))
        .collect();
    hits.sort_by_key(|c| std::cmp::Reverse(c.title.chars().count()));
    hits
}

/// Every `href` already present in the draft's inline HTML.
fn existing_hrefs(blocks: &[vyasa_core::block::Block]) -> Vec<String> {
    fn walk(blocks: &[vyasa_core::block::Block], out: &mut Vec<String>) {
        for b in blocks {
            for field in ["text", "summary", "caption"] {
                if let Some(t) = b.attrs.get(field).and_then(serde_json::Value::as_str) {
                    let mut rest = t;
                    while let Some(i) = rest.find("href=\"") {
                        rest = &rest[i + 6..];
                        if let Some(end) = rest.find('"') {
                            out.push(rest[..end].to_owned());
                            rest = &rest[end..];
                        }
                    }
                }
            }
            walk(&b.children, out);
        }
    }
    let mut out = Vec::new();
    walk(blocks, &mut out);
    out
}

/// Most suggestions returned in one call.
const MAX_SUGGESTIONS: usize = 8;

/// `POST /api/v1/posts/link-suggestions` — where should this draft link?
///
/// # Errors
///
/// 401 when unauthenticated.
#[utoipa::path(
    post, path = "/api/v1/posts/link-suggestions", tag = "posts",
    security(("session_cookie" = [])),
    request_body = LinkSuggestRequest,
    responses(
        (status = 200, description = "Suggestions, best first", body = [LinkSuggestion]),
        (status = 401, description = "Not authenticated", body = ApiErrorBody),
    )
)]
pub async fn suggest(
    State(state): State<AppState>,
    _principal: Principal,
    Json(body): Json<LinkSuggestRequest>,
) -> ApiResult<Json<Vec<LinkSuggestion>>> {
    let pattern = crate::permalinks::pattern(&state).await;
    let doc: vyasa_core::BlockDocument = serde_json::from_value(body.content)
        .unwrap_or_else(|_| vyasa_core::BlockDocument::new(Vec::new()));
    let text_lower = vyasa_themes::blocks_to_text(&doc.blocks).to_lowercase();
    let hrefs = existing_hrefs(&doc.blocks);

    // Link targets are public URLs: only types the public may open.
    let filter = PostFilter {
        status: Some(PostStatus::Published),
        limit: 500,
        readable_types: crate::policy::readable_custom_types(&state, None).await,
        ..PostFilter::default()
    };
    let rows = state.posts.list(&filter).await?;
    let candidates: Vec<Candidate> = rows
        .iter()
        .filter(|p| crate::public::page_meta::is_public_type(p.post_type))
        .filter(|p| p.password_hash.is_none())
        .filter(|p| Some(p.id) != body.exclude_post_id)
        .map(|p| Candidate {
            id: p.id,
            title: p.title.clone(),
            url: pattern.path_for(p),
        })
        .collect();

    let mut out: Vec<LinkSuggestion> = phrase_matches(&text_lower, &hrefs, &candidates)
        .into_iter()
        .take(MAX_SUGGESTIONS)
        .map(|c| LinkSuggestion {
            phrase: Some(c.title.clone()),
            post_id: c.id,
            title: c.title.clone(),
            url: c.url.clone(),
        })
        .collect();

    // Pad with embedding neighbours: no phrase to wrap, but worth a
    // "further reading" link. Requires a saved draft and the embeddings
    // feature; silently absent otherwise.
    if out.len() < MAX_SUGGESTIONS {
        if let Some(id) = body.exclude_post_id {
            let want = MAX_SUGGESTIONS - out.len();
            if let Ok(related) = crate::ai_features::related_posts(&state, id, want).await {
                for p in related {
                    let url = pattern.path_for(&p);
                    if out.iter().any(|s| s.post_id == p.id) || hrefs.contains(&url) {
                        continue;
                    }
                    out.push(LinkSuggestion {
                        phrase: None,
                        post_id: p.id,
                        title: p.title.clone(),
                        url,
                    });
                }
            }
        }
    }
    Ok(Json(out))
}

#[cfg(test)]
mod tests {
    use super::{existing_hrefs, phrase_matches, Candidate};

    fn c(id: i64, title: &str, url: &str) -> Candidate {
        Candidate {
            id,
            title: title.into(),
            url: url.into(),
        }
    }

    #[test]
    fn titles_found_in_the_draft_win_longest_first() {
        let candidates = vec![
            c(1, "Theme Studio", "/post/theme-studio"),
            c(2, "The Theme Studio Guide", "/post/guide"),
            c(3, "Menus", "/post/menus"), // too short to ever match
            c(4, "Absent Title", "/post/absent"),
        ];
        let text = "we rebuilt the theme studio guide and the theme studio itself";
        let hits = phrase_matches(text, &[], &candidates);
        let ids: Vec<i64> = hits.iter().map(|c| c.id).collect();
        assert_eq!(ids, [2, 1], "longest first, absent and short excluded");
    }

    #[test]
    fn already_linked_targets_are_not_suggested_again() {
        let candidates = vec![c(1, "Theme Studio", "/post/theme-studio")];
        let hits = phrase_matches(
            "all about the theme studio",
            &["/post/theme-studio".to_owned()],
            &candidates,
        );
        assert!(hits.is_empty());
    }

    #[test]
    fn hrefs_are_read_out_of_inline_html_at_any_depth() {
        let blocks: Vec<vyasa_core::block::Block> = serde_json::from_value(serde_json::json!([
            {"kind": "group", "attrs": {}, "children": [
                {"kind": "paragraph", "attrs": {"text": "see <a href=\"/post/a\">a</a> and <a href=\"/b\">b</a>"}, "children": []}
            ]}
        ]))
        .expect("blocks");
        assert_eq!(existing_hrefs(&blocks), ["/post/a", "/b"]);
    }
}
