//! `GET /api/v1/embeds/preview` — what an embed URL points at, for the
//! editor's card.
//!
//! The embed block renders as an iframe for the two providers the
//! renderer allows (YouTube, Vimeo) and as a bare link for anything else.
//! In the editor it was a card that said only the URL. Both providers
//! publish oEmbed, so the card can show the title and a thumbnail. The
//! request goes to the provider's own oEmbed endpoint, never to the URL
//! the author typed, and only for a URL the renderer would embed.

use axum::extract::{Query, State};
use axum::Json;
use serde::{Deserialize, Serialize};
use vyasa_common::AppError;

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// Query: the URL an author put in an embed block.
#[derive(Deserialize, utoipa::IntoParams)]
pub struct PreviewQuery {
    /// The embed URL.
    pub url: String,
}

/// What the provider says about the URL.
#[derive(Serialize, utoipa::ToSchema)]
pub struct EmbedPreview {
    /// `youtube` or `vimeo`.
    pub provider: String,
    /// Title, when the provider gave one.
    pub title: Option<String>,
    /// Thumbnail URL, when the provider gave one.
    pub thumbnail_url: Option<String>,
    /// Author or channel name.
    pub author_name: Option<String>,
}

const MAX_BODY: usize = 256 * 1024;

/// `GET /api/v1/embeds/preview?url=…`
#[utoipa::path(
    get, path = "/api/v1/embeds/preview", tag = "posts",
    security(("session_cookie" = [])),
    params(PreviewQuery),
    responses(
        (status = 200, description = "Provider metadata", body = EmbedPreview),
        (status = 400, description = "Not a URL the site embeds"),
        (status = 502, description = "The provider did not answer"),
    )
)]
pub async fn preview(
    State(state): State<AppState>,
    Query(q): Query<PreviewQuery>,
) -> ApiResult<Json<EmbedPreview>> {
    let vyasa_themes::EmbedOutcome::Iframe { provider, .. } =
        vyasa_themes::resolve_embed_url(&q.url)
    else {
        return Err(ApiError(AppError::validation(
            "only YouTube and Vimeo links are embedded; anything else renders as a link",
        )));
    };
    let endpoint = match provider {
        "youtube" => "https://www.youtube.com/oembed",
        "vimeo" => "https://vimeo.com/api/oembed.json",
        _ => {
            return Err(ApiError(AppError::validation(
                "no preview for this provider",
            )))
        }
    };
    let target =
        reqwest::Url::parse_with_params(endpoint, [("url", q.url.as_str()), ("format", "json")])
            .map_err(|e| AppError::validation(format!("bad embed url: {e}")))?;
    let response = state
        .embed_client
        .get(target)
        .send()
        .await
        .map_err(|e| AppError::external("oembed", format!("{provider}: {e}")))?;
    if !response.status().is_success() {
        return Err(ApiError(AppError::external(
            "oembed",
            format!("{provider} answered {}", response.status()),
        )));
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|e| AppError::external("oembed", e.to_string()))?;
    if bytes.len() > MAX_BODY {
        return Err(ApiError(AppError::external(
            "oembed",
            "the provider's answer was too large",
        )));
    }
    let body: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|e| AppError::external("oembed", format!("unreadable answer: {e}")))?;
    let text = |key: &str| {
        body.get(key)
            .and_then(serde_json::Value::as_str)
            .map(|s| s.chars().take(300).collect::<String>())
    };
    // Only an https thumbnail from the provider's answer is passed on.
    let thumbnail_url = text("thumbnail_url").filter(|u| u.starts_with("https://"));
    Ok(Json(EmbedPreview {
        provider: provider.to_owned(),
        title: text("title"),
        thumbnail_url,
        author_name: text("author_name"),
    }))
}
