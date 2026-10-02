//! The studio assistant as a job: a message on a draft becomes a run of
//! the planner, a revision (when something changed) and a reply in the
//! conversation. Runs through the queue because a plan can take a
//! minute, longer than a request through a tunnel should hold.

use serde_json::{json, Value};
use vyasa_ai::theme_studio::AssistInput;
use vyasa_ai::Attachment;
use vyasa_common::AppError;
use vyasa_core::ai_models::ModelKind;
use vyasa_db::repo::ThemeDraftsRepo;
use vyasa_themes::DraftState;

use crate::ai_registry;
use crate::state::AppState;

/// The job kind.
pub const JOB_KIND: &str = "ai_theme_chat";
/// Images larger than this are not sent to a vision model.
const MAX_IMAGE_BYTES: usize = 4 * 1024 * 1024;

/// Draft status while the assistant works.
pub const STATUS_GENERATING: &str = "generating";
/// Draft status when the last run failed; `status_note` says why.
pub const STATUS_FAILED: &str = "failed";
/// Draft status otherwise.
pub const STATUS_READY: &str = "ready";

fn repo(state: &AppState) -> ThemeDraftsRepo {
    ThemeDraftsRepo::new(state.pool.clone())
}

/// Records the author's message, marks the draft as generating and
/// queues the run. Fails up front when no usable model exists, so the
/// author hears that immediately rather than from a failed job.
///
/// # Errors
/// [`AppError::Validation`] when the message is empty or no model is
/// registered; [`AppError::NotFound`] for an unknown draft.
pub async fn ask(
    state: &AppState,
    draft_id: i64,
    message: &str,
    media_ids: &[i64],
) -> Result<i64, AppError> {
    let message = message.trim();
    if message.is_empty() {
        return Err(AppError::validation("say what you would like changed"));
    }
    if message.chars().count() > 4000 {
        return Err(AppError::validation("keep a request under 4000 characters"));
    }
    let repo = repo(state);
    let draft = repo.get(draft_id).await?;
    if draft.status == STATUS_GENERATING {
        return Err(AppError::conflict(
            "the assistant is still working on the last request",
        ));
    }
    // A missing model is a validation error with instructions.
    let kind = if media_ids.is_empty() {
        ModelKind::Text
    } else {
        ModelKind::Vision
    };
    ai_registry::chain(state, kind).await?;

    let row = repo.add_message(draft_id, "you", message, None).await?;
    repo.set_status(draft_id, STATUS_GENERATING, None).await?;
    crate::ai_jobs::enqueue(
        state,
        JOB_KIND,
        json!({ "draft_id": draft_id, "message_id": row.id, "media_ids": media_ids }),
    )
    .await;
    Ok(row.id)
}

/// The job body: plan, apply, record.
///
/// # Errors
/// Propagates provider and validation failures after recording them on
/// the draft, so the queue's retry does not re-run a request the author
/// can already see failed.
pub async fn run(
    state: &AppState,
    draft_id: i64,
    message_id: i64,
    media_ids: &[i64],
) -> Result<(), AppError> {
    let outcome = run_inner(state, draft_id, message_id, media_ids).await;
    let repo = repo(state);
    match outcome {
        Ok(()) => Ok(()),
        Err(err) => {
            tracing::warn!(draft_id, error = %err, "theme assistant run failed");
            // What the author reads. The line above is what an operator reads,
            // and it keeps the category and the provider wording behind it.
            let note = err.client_message();
            let _ = repo
                .add_message(
                    draft_id,
                    "assistant",
                    &format!("I couldn't do that: {note}"),
                    None,
                )
                .await;
            let _ = repo.set_status(draft_id, STATUS_FAILED, Some(&note)).await;
            // The failure is recorded where the author sees it; the queue
            // must not retry a run that cost money and already answered.
            Ok(())
        }
    }
}

async fn run_inner(
    state: &AppState,
    draft_id: i64,
    message_id: i64,
    media_ids: &[i64],
) -> Result<(), AppError> {
    let repo = repo(state);
    let draft = repo.get(draft_id).await?;
    let history = repo.messages(draft_id).await?;
    let message = history
        .iter()
        .find(|m| m.id == message_id)
        .map(|m| m.text.clone())
        .ok_or_else(|| AppError::not_found("message", message_id))?;
    let earlier: Vec<(String, String)> = history
        .iter()
        .filter(|m| m.id != message_id)
        .map(|m| (m.role.clone(), m.text.clone()))
        .collect();

    let attachments = load_images(state, media_ids).await?;
    let kind = if attachments.is_empty() {
        ModelKind::Text
    } else {
        ModelKind::Vision
    };
    let provider = ai_registry::chain(state, kind).await?;
    let sink = ai_registry::sink(state).await;

    let current = DraftState::from_json_with_assets(
        &draft.tokens,
        &draft.layout,
        draft.templates.as_ref(),
        draft.assets.as_ref(),
    )
    .map_err(|e| AppError::internal_msg(format!("stored draft is unreadable: {e}")))?;
    let identity = state.options_service.site_identity().await?;
    let site = if identity.tagline.is_empty() {
        identity.title
    } else {
        format!("{} — {}", identity.title, identity.tagline)
    };
    let brand = state.options_service.brand_kit_prompt().await?;
    let vocabulary = crate::rest::content_types::vocabulary_with_sources(state).await?;
    let input = AssistInput {
        vocabulary: &vocabulary,
        site: &site,
        brand: &brand,
        history: &earlier,
        message: &message,
        attachments: attachments.clone(),
        model: provider.primary_model(),
    };
    // The agent works a sandbox copy of the draft: edit, look at the
    // rendered page, repair what the look shows. Nothing persists until
    // a person accepts.
    let system = vyasa_ai::theme_studio::system_prompt(&vocabulary);
    let task = vyasa_ai::theme_studio::user_prompt(&current, &input);
    let registry = crate::plugin_sections::registry_with_plugins(state).await;
    let mut toolbox = vyasa_ai::theme_studio::StudioToolbox::new(
        &current,
        registry,
        live_menu_drafts(state).await?,
        Some(crate::agent_eyes::LiveStudioEyes {
            state: state.clone(),
            base_theme_id: draft.base_theme_id,
        }),
    )
    .allow_script(vyasa_ai::theme_studio::request_allows_script(&message));
    let outcome = vyasa_ai::agent::run_agent(
        &provider,
        &sink,
        &vyasa_ai::agent::AgentSpec {
            purpose: "theme-studio",
            model: provider.primary_model(),
            system: &system,
            task: &task,
            max_steps: 12,
            attachments,
        },
        &mut toolbox,
    )
    .await?;

    // The result is offered, not written: the sandbox's end state waits
    // on the message until someone accepts it, with the steps alongside
    // so the author can see the work, not just the answer.
    let proposal = build_proposal(&toolbox, &outcome.steps);
    let reply = if outcome.reply.is_empty() {
        if proposal.is_some() {
            "Here is what I would change.".to_owned()
        } else {
            "Nothing needed changing.".to_owned()
        }
    } else {
        outcome.reply
    };
    repo.add_reply(
        draft_id,
        "assistant",
        &reply,
        None,
        proposal.as_ref(),
        proposal.is_some().then_some(draft.revision),
    )
    .await?;
    repo.set_status(draft_id, STATUS_READY, None).await?;
    Ok(())
}

/// The sandbox's end state as a proposal, or `None` when the run changed
/// nothing. Staged navigation rides along — only the menus the agent
/// touched, applied through `MenuService` when the person accepts.
fn build_proposal(
    toolbox: &vyasa_ai::theme_studio::StudioToolbox<crate::agent_eyes::LiveStudioEyes>,
    steps: &[vyasa_ai::agent::AgentStep],
) -> Option<Value> {
    (!toolbox.changes.is_empty()).then(|| {
        let mut p = serde_json::json!({
            "tokens": toolbox.sandbox.tokens_json(),
            "layout": toolbox.sandbox.layout_json(),
            "templates": toolbox.sandbox.templates_json(),
            "assets": toolbox.sandbox.assets_json(),
            "changes": toolbox.changes,
            "steps": serde_json::to_value(steps).unwrap_or_default(),
        });
        let staged: Vec<&vyasa_core::menu::MenuDraft> = toolbox
            .menus
            .iter()
            .filter(|m| toolbox.changed_menus.contains(&m.slug))
            .collect();
        if !staged.is_empty() {
            p["menus"] = serde_json::to_value(&staged).unwrap_or_default();
        }
        p
    })
}

/// The site's menus as drafts — the agent's navigation sandbox starts
/// from what actually exists.
pub async fn live_menu_drafts(
    state: &AppState,
) -> Result<Vec<vyasa_core::menu::MenuDraft>, AppError> {
    let mut out = Vec::new();
    for menu in state.menus.list().await? {
        let items = state.menus.items(menu.id).await?;
        out.push(vyasa_core::menu::MenuDraft::from_rows(&menu, &items));
    }
    Ok(out)
}

async fn load_images(state: &AppState, media_ids: &[i64]) -> Result<Vec<Attachment>, AppError> {
    use base64::Engine as _;
    let mut out = Vec::with_capacity(media_ids.len());
    for id in media_ids.iter().take(4) {
        let row = state.media.get(*id).await?;
        if !row.mime.starts_with("image/") {
            return Err(AppError::validation(format!(
                "{} is not an image",
                row.file_name
            )));
        }
        let bytes = state.media.get_bytes(&row).await?;
        if bytes.len() > MAX_IMAGE_BYTES {
            return Err(AppError::validation(format!(
                "{} is too large to send to a vision model (4 MB limit)",
                row.file_name
            )));
        }
        out.push(Attachment {
            mime: row.mime.clone(),
            data_b64: base64::engine::general_purpose::STANDARD.encode(bytes),
        });
    }
    Ok(out)
}

/// Payload fields the job carries.
#[must_use]
pub fn payload_ids(payload: &Value) -> Vec<i64> {
    payload
        .get("media_ids")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_i64).collect())
        .unwrap_or_default()
}
