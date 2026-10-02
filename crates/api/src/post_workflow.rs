//! Side effects shared by every HTTP post-save surface.
use crate::state::AppState;
use vyasa_db::content_models::{PostRow, PostStatus};

/// Records history, address changes, scheduling and named plugin hooks.
pub async fn after_save(
    state: &AppState,
    author_id: i64,
    previous: Option<&PostRow>,
    post: &PostRow,
) {
    if let Some(old) = previous.filter(|p| crate::seo::is_live(p.status)) {
        let pattern = crate::permalinks::pattern(state).await;
        let from = pattern.path_for(old);
        let to = pattern.path_for(post);
        if from != to {
            if let Err(e) = crate::seo::add_redirect(&state.pool, &from, &to).await {
                tracing::warn!("could not record redirect: {e}");
            }
        }
    }
    if let Err(e) = state
        .revisions
        .snapshot_if_changed(post.id, author_id, &vyasa_core::post::Snapshot::of(post))
        .await
    {
        tracing::warn!("could not snapshot post: {e}");
    }
    let revisions = state.revisions.clone();
    let id = post.id;
    tokio::spawn(async move {
        if let Err(e) = revisions.prune(id, chrono::Utc::now()).await {
            tracing::warn!("could not prune revisions: {e}");
        }
    });
    if post.status == PostStatus::Scheduled {
        state.publisher_notify.notify_one();
    }
    crate::plugin_hooks::emit(
        state,
        crate::plugin_hooks::events::POST_SAVED,
        serde_json::json!({"post_id":post.id,"status":post.status.as_str(),"post_type":post.post_type.as_str()}),
    );
}
