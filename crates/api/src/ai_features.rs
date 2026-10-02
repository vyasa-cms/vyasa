//! The AI integrations: what each registered model kind does for the site.
//!
//! Every function here is the whole of one feature — read the inputs, ask
//! the registry's model for that kind, write the result — so the job
//! handler, the event subscriber and the REST endpoints are thin. Each is
//! gated by an option an operator turns on; nothing here runs by default.

use std::collections::HashMap;

use base64::Engine as _;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use vyasa_ai::assist::{extract_text, AssistKind};
use vyasa_ai::{Attachment, OpenAiProvider, PromptSpec};
use vyasa_common::AppError;
use vyasa_core::ai_models::{ModelKind, ProviderId, ResolvedModel};
use vyasa_core::post::service::UpdatePost;
use vyasa_db::content_models::{CommentStatus, MediaRow, PostRow, PostStatus};
use vyasa_db::repo::AiDataRepo;

use crate::ai_registry;
use crate::state::AppState;

/// Longest text sent to an embedding or speech model, in characters.
const EMBED_CHARS: usize = 8000;
const SPEECH_CHUNK_CHARS: usize = 3800;
const SPEECH_MAX_CHARS: usize = 30_000;

/// Comment screening modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screening {
    /// Do nothing.
    Off,
    /// Record the verdict, leave the status alone.
    Flag,
    /// Flagged comments go straight to spam.
    Spam,
}

/// The operator's feature switches — independent toggles, not a state
/// machine, which is why they are bools.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Copy)]
pub struct AiSettings {
    pub alt_text: bool,
    pub comment_screening: Screening,
    pub autofill: bool,
    pub embeddings: bool,
    pub semantic_search: bool,
    pub related_posts: bool,
    pub images: bool,
    pub transcription: bool,
    pub read_aloud: bool,
}

impl AiSettings {
    /// Reads the switches; a missing option is off.
    pub async fn load(state: &AppState) -> Self {
        let flag = |key: &'static str| async move {
            state
                .options
                .get_or_default(key, Value::Bool(false))
                .await
                .ok()
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
        };
        let screening = match state
            .options
            .get_or_default("ai_comment_screening", json!("off"))
            .await
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .as_deref()
        {
            Some("flag") => Screening::Flag,
            Some("spam") => Screening::Spam,
            _ => Screening::Off,
        };
        Self {
            alt_text: flag("ai_alt_text").await,
            comment_screening: screening,
            autofill: flag("ai_autofill").await,
            embeddings: flag("ai_embeddings").await,
            semantic_search: flag("ai_semantic_search").await,
            related_posts: flag("ai_related_posts").await,
            images: flag("ai_images").await,
            transcription: flag("ai_transcription").await,
            read_aloud: flag("ai_read_aloud").await,
        }
    }
}

/// Reply shapes the structured runner validates.
#[derive(serde::Deserialize, schemars::JsonSchema)]
struct AltReply {
    alt: String,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct Suggestions {
    suggestions: Vec<String>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct Seo {
    meta_title: String,
    meta_description: String,
}

fn data_repo(state: &AppState) -> AiDataRepo {
    AiDataRepo::new(state.pool.clone())
}

fn text_hash(text: &str) -> String {
    hex::encode(Sha256::digest(text.as_bytes()))
}

/// Title plus body text, the way every text-consuming feature sees a post.
fn post_text(post: &PostRow, budget: usize) -> String {
    let blocks: Vec<vyasa_core::block::Block> = post
        .content
        .get("blocks")
        .cloned()
        .and_then(|b| serde_json::from_value(b).ok())
        .unwrap_or_default();
    let body = extract_text(&blocks, budget);
    format!("{}\n\n{body}", post.title)
}

/// An OpenAI-compatible client for a resolved model — the kinds beyond chat
/// (embeddings, moderation, images, audio) only exist at those providers.
fn compatible_client(model: &ResolvedModel) -> Result<OpenAiProvider, AppError> {
    if model.provider == ProviderId::Anthropic {
        return Err(AppError::validation(format!(
            "Anthropic does not offer {} models",
            model.row.kind
        )));
    }
    Ok(
        vyasa_ai::catalog::compatible(model.provider, &model.api_key, &model.base_url)?
            .with_settings(&model.row.settings),
    )
}

fn external(model: &ResolvedModel, e: &vyasa_ai::ProviderError) -> AppError {
    AppError::external(model.provider.as_str(), format!("{}: {e}", model.row.model))
}

/* ------------------------------------------------------------- vision */

/// Writes alt text (and a caption when the image has none) for a media row
/// using the vision default. Skips non-images and images that already have
/// alt text unless `force` is set.
pub async fn alt_text(
    state: &AppState,
    media_id: i64,
    force: bool,
) -> Result<Option<String>, AppError> {
    let row = state.media.get(media_id).await?;
    if !row.mime.starts_with("image/") {
        return Err(AppError::validation("only images get alt text"));
    }
    if !force && row.alt.as_deref().is_some_and(|a| !a.trim().is_empty()) {
        return Ok(None);
    }
    let bytes = state.media.get_bytes(&row).await?;
    let provider = ai_registry::chain(state, ModelKind::Vision).await?;
    let spec = PromptSpec {
        attachments: vec![Attachment {
            mime: row.mime.clone(),
            data_b64: base64::engine::general_purpose::STANDARD.encode(&bytes),
        }],
        purpose: String::from("alt-text"),
        model: provider.primary_model().to_owned(),
        system: String::from(
            "You write alt text for images on a website. Describe what the image shows for \
             someone who cannot see it: concrete, specific, one sentence, at most 125 \
             characters, no leading 'image of'. If text is visible and matters, include it.",
        ),
        user: format!("File name: {}. Write the alt text.", row.file_name),
        output_schema: json!({
            "type": "object",
            "properties": {"alt": {"type": "string", "maxLength": 200}},
            "required": ["alt"],
            "additionalProperties": false
        }),
    };
    let sink = ai_registry::sink(state).await;
    let out: AltReply = vyasa_ai::run_structured(&provider, &sink, &spec).await?;
    let alt = out.alt.trim().to_owned();
    if alt.is_empty() {
        return Err(AppError::external(
            "ai",
            "the model returned empty alt text",
        ));
    }
    state
        .media
        .repo()
        .update_meta(media_id, Some(&alt), None)
        .await?;
    Ok(Some(alt))
}

/* --------------------------------------------------------- moderation */

/// Screens a comment with the moderation default and records the verdict;
/// in `spam` mode a flagged comment is moved to spam.
pub async fn moderate_comment(
    state: &AppState,
    comment_id: i64,
    mode: Screening,
) -> Result<Value, AppError> {
    if mode == Screening::Off {
        return Ok(json!({ "skipped": "off" }));
    }
    let comment = state.comments.get(comment_id).await?;
    let model = ai_registry::first(state, ModelKind::Moderation).await?;
    let client = compatible_client(&model)?;
    let text = format!("{}\n{}", comment.author_name, comment.content);
    let verdict = client
        .moderate(&model.row.model, &text)
        .await
        .map_err(|e| external(&model, &e))?;
    let top: Vec<Value> = verdict
        .scores
        .iter()
        .take(3)
        .map(|(category, score)| json!({ "category": category, "score": (score * 1000.0).round() / 1000.0 }))
        .collect();
    let mut action = "none";
    if verdict.flagged && mode == Screening::Spam && comment.status == CommentStatus::Pending {
        state
            .comments
            .moderate(comment_id, CommentStatus::Spam)
            .await?;
        action = "spam";
    }
    let record = json!({
        "flagged": verdict.flagged,
        "top": top,
        "model": model.row.model,
        "action": action,
        "at": chrono::Utc::now(),
    });
    data_repo(state)
        .set_comment_moderation(comment_id, &record)
        .await?;
    Ok(record)
}

/* --------------------------------------------------------------- text */

/// Fills an empty excerpt and an empty SEO description from the text
/// default. Fields that already have a value are left alone.
pub async fn autofill_post(state: &AppState, post_id: i64) -> Result<Value, AppError> {
    let post = state.posts.get(post_id).await?;
    let needs_excerpt = post.excerpt.as_deref().is_none_or(|e| e.trim().is_empty());
    let needs_seo = post.meta["seo_description"]
        .as_str()
        .is_none_or(|s| s.trim().is_empty());
    if !needs_excerpt && !needs_seo {
        return Ok(json!({ "filled": [] }));
    }
    let text = extract_text_of(&post, 4000);
    if !vyasa_ai::assist::enough_content(&text) {
        return Ok(json!({ "filled": [], "reason": "not enough content" }));
    }
    let provider = ai_registry::chain(state, ModelKind::Text).await?;
    let sink = ai_registry::sink(state).await;
    let mut filled = Vec::new();
    let mut update = UpdatePost::default();

    if needs_excerpt {
        let mut spec = AssistKind::Excerpt.prompt_spec(&text, &[]);
        spec.model = provider.primary_model().to_owned();
        let out: Suggestions = vyasa_ai::run_structured(&provider, &sink, &spec).await?;
        if let Some(first) = out.suggestions.into_iter().find(|s| !s.trim().is_empty()) {
            update.excerpt = Some(first.trim().to_owned());
            filled.push("excerpt");
        }
    }
    if needs_seo {
        let mut spec = AssistKind::Seo.prompt_spec(&text, &[]);
        spec.model = provider.primary_model().to_owned();
        let out: Seo = vyasa_ai::run_structured(&provider, &sink, &spec).await?;
        let mut meta = post.meta.clone();
        if !meta.is_object() {
            meta = json!({});
        }
        meta["seo_description"] = json!(out.meta_description.trim());
        if meta["seo_title"].as_str().is_none_or(str::is_empty) {
            meta["seo_title"] = json!(out.meta_title.trim());
        }
        update.meta = Some(meta);
        filled.push("seo_description");
    }
    if !filled.is_empty() {
        state.posts.update(post_id, update).await?;
    }
    Ok(json!({ "filled": filled }))
}

fn extract_text_of(post: &PostRow, budget: usize) -> String {
    let blocks: Vec<vyasa_core::block::Block> = post
        .content
        .get("blocks")
        .cloned()
        .and_then(|b| serde_json::from_value(b).ok())
        .unwrap_or_default();
    extract_text(&blocks, budget)
}

/* ---------------------------------------------------------- embeddings */

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let (mut dot, mut na, mut nb) = (0.0f32, 0.0f32, 0.0f32);
    for (x, y) in a.iter().zip(b) {
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na.sqrt() * nb.sqrt())
    }
}

async fn embed_texts(
    state: &AppState,
    texts: &[String],
) -> Result<(String, Vec<Vec<f32>>), AppError> {
    let model = ai_registry::first(state, ModelKind::Embedding).await?;
    let client = compatible_client(&model)?;
    let vectors = client
        .embed(&model.row.model, texts)
        .await
        .map_err(|e| external(&model, &e))?;
    Ok((model.row.model, vectors))
}

/// Embeds a post if its text changed since the stored vector. Unpublished
/// posts lose their vector so they never surface as related.
pub async fn embed_post(state: &AppState, post_id: i64) -> Result<Value, AppError> {
    let post = state.posts.get(post_id).await?;
    let repo = data_repo(state);
    if post.status != PostStatus::Published {
        repo.delete_embedding(post_id).await?;
        return Ok(json!({ "embedded": false, "reason": "not published" }));
    }
    let text = post_text(&post, EMBED_CHARS);
    let hash = text_hash(&text);
    if let Some(existing) = repo.embedding(post_id).await? {
        if existing.text_hash == hash {
            return Ok(json!({ "embedded": false, "reason": "unchanged" }));
        }
    }
    let (model, vectors) = embed_texts(state, std::slice::from_ref(&text)).await?;
    let Some(vector) = vectors.into_iter().next() else {
        return Err(AppError::external("ai", "no embedding returned"));
    };
    repo.upsert_embedding(post_id, &model, &hash, &vector)
        .await?;
    Ok(json!({ "embedded": true, "dims": vector.len(), "model": model }))
}

/// Ids of the published posts nearest to `post_id`, best first.
///
/// `public_custom`: the custom types served publicly; nothing else (no
/// protected entry, no block, no non-public type) is ever a neighbour.
pub async fn related_ids(
    repo: &AiDataRepo,
    post_id: i64,
    limit: usize,
    public_custom: &[String],
) -> Result<Vec<i64>, AppError> {
    let Some(own) = repo.embedding(post_id).await? else {
        return Ok(Vec::new());
    };
    let mut scored: Vec<(f32, i64)> = repo
        .published_embeddings(own.dims, Some(public_custom))
        .await?
        .into_iter()
        .filter(|row| row.post_id != post_id)
        .map(|row| (cosine(&own.vector, &row.vector), row.post_id))
        .collect();
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    Ok(scored.into_iter().take(limit).map(|(_, id)| id).collect())
}

/// Published posts nearest to `post_id`, best first.
pub async fn related_posts(
    state: &AppState,
    post_id: i64,
    limit: usize,
) -> Result<Vec<PostRow>, AppError> {
    let public_custom = crate::policy::PublicTypes::load(&state.plugin_surface, &state.pool).await;
    let slugs: Vec<String> = public_custom.slugs();
    let mut out = Vec::new();
    for id in related_ids(&data_repo(state), post_id, limit, &slugs).await? {
        if let Ok(post) = state.posts.get(id).await {
            // Re-checked on the row: the embedding listing ran a moment ago.
            if post.status == PostStatus::Published
                && post.password_hash.is_none()
                && public_custom.opens(post.post_type)
            {
                out.push(post);
            }
        }
    }
    Ok(out)
}

/// Re-orders full-text candidates by meaning: the query is embedded once
/// and the candidates that have vectors are sorted by similarity, ahead of
/// those that do not. Any failure returns the original order — search must
/// never break because a model did.
///
/// `principal` decides which entries' vectors take part: the public's set
/// for a visitor, every type for someone who edits content (their search
/// candidates may include entries the public cannot open).
pub async fn semantic_rerank(
    state: &AppState,
    principal: Option<&crate::middleware::Principal>,
    query: &str,
    candidates: Vec<i64>,
) -> Vec<i64> {
    if candidates.len() < 2 {
        return candidates;
    }
    let Ok((_, vectors)) = embed_texts(state, &[query.to_owned()]).await else {
        return candidates;
    };
    let Some(q) = vectors.into_iter().next() else {
        return candidates;
    };
    let dims = i32::try_from(q.len()).unwrap_or(0);
    let readable = crate::policy::readable_custom_types(state, principal).await;
    let Ok(rows) = data_repo(state)
        .published_embeddings(dims, readable.as_deref())
        .await
    else {
        return candidates;
    };
    let by_id: HashMap<i64, Vec<f32>> = rows.into_iter().map(|r| (r.post_id, r.vector)).collect();
    let mut with: Vec<(f32, i64)> = Vec::new();
    let mut without: Vec<i64> = Vec::new();
    for id in candidates {
        match by_id.get(&id) {
            Some(v) => with.push((cosine(&q, v), id)),
            None => without.push(id),
        }
    }
    with.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    with.into_iter().map(|(_, id)| id).chain(without).collect()
}

/* --------------------------------------------------------------- image */

/// Generates an image with the image default and files it in the media
/// library, owned by the requesting user, with the prompt as alt text.
pub async fn generate_image(
    state: &AppState,
    owner_id: i64,
    prompt: &str,
    size: &str,
) -> Result<MediaRow, AppError> {
    let prompt = prompt.trim();
    if prompt.chars().count() < 3 {
        return Err(AppError::validation("describe the image you want"));
    }
    let size = match size {
        "1024x1024" | "1024x1536" | "1536x1024" | "1024x1792" | "1792x1024" => size,
        _ => "1024x1024",
    };
    let model = ai_registry::first(state, ModelKind::Image).await?;
    let client = compatible_client(&model)?;
    let bytes = client
        .generate_image(&model.row.model, prompt, size)
        .await
        .map_err(|e| external(&model, &e))?;
    let name = format!(
        "generated-{}.png",
        chrono::Utc::now().format("%Y%m%d-%H%M%S")
    );
    let alt: String = prompt.chars().take(200).collect();
    let row = state
        .media
        .upload(
            owner_id,
            &name,
            bytes,
            Some(alt),
            Some(format!("Generated with {}", model.row.model)),
        )
        .await?;
    ai_registry::log_flat(state, &model, "image-generate").await;
    Ok(row)
}

/* ---------------------------------------------------------------- audio */

/// Transcribes an audio or video file and stores the text on the row.
pub async fn transcribe_media(state: &AppState, media_id: i64) -> Result<String, AppError> {
    let row = state.media.get(media_id).await?;
    if !(row.mime.starts_with("audio/") || row.mime.starts_with("video/")) {
        return Err(AppError::validation(
            "only audio and video files can be transcribed",
        ));
    }
    let bytes = state.media.get_bytes(&row).await?;
    let model = ai_registry::first(state, ModelKind::Transcription).await?;
    let client = compatible_client(&model)?;
    let text = client
        .transcribe(&model.row.model, bytes, &row.file_name, &row.mime)
        .await
        .map_err(|e| external(&model, &e))?;
    if text.is_empty() {
        return Err(AppError::external("ai", "the transcript came back empty"));
    }
    data_repo(state).set_transcript(media_id, &text).await?;
    ai_registry::log_flat(state, &model, "transcribe").await;
    Ok(text)
}

/// Splits text into chunks a speech model accepts, on sentence boundaries
/// where possible.
fn speech_chunks(text: &str) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();
    for sentence in text.split_inclusive(['.', '!', '?', '\n']) {
        if current.chars().count() + sentence.chars().count() > SPEECH_CHUNK_CHARS
            && !current.trim().is_empty()
        {
            chunks.push(std::mem::take(&mut current));
        }
        current.push_str(sentence);
    }
    if !current.trim().is_empty() {
        chunks.push(current);
    }
    chunks
}

/// Reads a post aloud with the speech default and files the MP3 in the
/// media library; unchanged text keeps the existing recording.
pub async fn read_aloud(state: &AppState, post_id: i64) -> Result<Value, AppError> {
    let post = state.posts.get(post_id).await?;
    let repo = data_repo(state);
    let text: String = post_text(&post, SPEECH_MAX_CHARS)
        .chars()
        .take(SPEECH_MAX_CHARS)
        .collect();
    let hash = text_hash(&text);
    if let Some(existing) = repo.post_audio(post_id).await? {
        if existing.text_hash == hash && state.media.get(existing.media_id).await.is_ok() {
            return Ok(json!({ "generated": false, "media_id": existing.media_id.to_string() }));
        }
    }
    let model = ai_registry::first(state, ModelKind::Speech).await?;
    let client = compatible_client(&model)?;
    let voice = model.row.settings["voice"]
        .as_str()
        .unwrap_or("alloy")
        .to_owned();
    let mut audio = Vec::new();
    for chunk in speech_chunks(&text) {
        let part = client
            .speak(&model.row.model, &chunk, &voice)
            .await
            .map_err(|e| external(&model, &e))?;
        audio.extend_from_slice(&part);
    }
    if audio.is_empty() {
        return Err(AppError::external("ai", "no audio returned"));
    }
    let name = format!("read-aloud-{}.mp3", post.slug);
    let row = state
        .media
        .upload(
            post.author_id,
            &name,
            audio,
            Some(format!("Audio version of “{}”", post.title)),
            None,
        )
        .await?;
    repo.set_post_audio(post_id, row.id, &model.row.model, &hash)
        .await?;
    ai_registry::log_flat(state, &model, "read-aloud").await;
    Ok(json!({ "generated": true, "media_id": row.id.to_string() }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cosine_is_one_for_parallel_and_zero_for_orthogonal_or_mismatched() {
        assert!((cosine(&[1.0, 2.0], &[2.0, 4.0]) - 1.0).abs() < 1e-6);
        assert!(cosine(&[1.0, 0.0], &[0.0, 1.0]).abs() < 1e-6);
        assert!(cosine(&[1.0], &[1.0, 1.0]).abs() < f32::EPSILON);
        assert!(cosine(&[0.0, 0.0], &[1.0, 1.0]).abs() < f32::EPSILON);
    }

    #[test]
    fn speech_chunks_split_on_sentences_under_the_limit() {
        let sentence = "This is a sentence. ";
        let text = sentence.repeat(400); // 8000 chars
        let chunks = speech_chunks(&text);
        assert!(chunks.len() >= 3);
        assert!(chunks
            .iter()
            .all(|c| c.chars().count() <= SPEECH_CHUNK_CHARS));
        assert!(chunks.iter().all(|c| c.trim_end().ends_with('.')));
        assert_eq!(chunks.concat(), text);
    }

    #[test]
    fn text_hash_is_stable() {
        assert_eq!(text_hash("a"), text_hash("a"));
        assert_ne!(text_hash("a"), text_hash("b"));
    }
}
