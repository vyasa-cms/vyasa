//! Emails post authors when their post gets a comment.
//!
//! Phase 44 shipped a `comment_notification` template that nothing ever
//! rendered. This subscribes to the domain event bus — the same bus the
//! search indexer, cache invalidator and webhook dispatcher use — and
//! queues one email per new comment.
//!
//! Authors can opt out: `users.meta->>'notify_comments' = "false"` silences
//! it. The default is on, matching what people expect from a CMS.

use vyasa_core::events::{self, Event};

use crate::state::AppState;

/// How much of the comment to quote in the email.
const EXCERPT_CHARS: usize = 300;

/// Subscribes to comment events and queues author notifications.
pub fn spawn(state: &AppState) {
    let state = state.clone();
    let mut rx = events::subscribe();

    tokio::spawn(async move {
        loop {
            let event = match rx.recv().await {
                Ok(event) => event,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!("comment notifier lagged {n} events; notices were lost");
                    continue;
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            };
            let Event::CommentAdded(added) = event else {
                continue;
            };
            if let Err(err) = notify(&state, added.comment_id, added.post_id).await {
                // A notification is never worth failing anything else over.
                tracing::warn!(
                    comment_id = added.comment_id,
                    "comment notification skipped: {err}"
                );
            }
        }
    });
}

/// Truncates on a character boundary, adding an ellipsis when cut.
fn excerpt(body: &str) -> String {
    let trimmed = body.trim();
    if trimmed.chars().count() <= EXCERPT_CHARS {
        return trimmed.to_owned();
    }
    let cut: String = trimmed.chars().take(EXCERPT_CHARS).collect();
    format!("{cut}…")
}

async fn notify(
    state: &AppState,
    comment_id: i64,
    post_id: i64,
) -> Result<(), vyasa_common::AppError> {
    let comment = vyasa_db::repo::CommentsRepo::new(state.pool.clone())
        .get(comment_id)
        .await?;
    let post = state.posts.get(post_id).await?;
    let author = vyasa_db::repo::UsersRepo::new(state.pool.clone())
        .get(post.author_id)
        .await?;

    // Do not email someone about their own comment.
    if comment.author_user_id == Some(author.id) {
        return Ok(());
    }
    if author
        .meta
        .get("notify_comments")
        .is_some_and(|v| v == &serde_json::Value::Bool(false))
    {
        return Ok(());
    }
    if author.email.is_empty() {
        return Ok(());
    }

    let base = match state.options_service.site_url().await {
        Ok(url) if !url.is_empty() => url,
        // Without a configured site URL there is no honest absolute link to
        // put in an email, and no request to borrow an origin from here.
        _ => {
            tracing::debug!("comment notification skipped: site_url is not configured");
            return Ok(());
        }
    };
    let ctx = serde_json::json!({
        "name": author.display_name,
        "post_title": post.title,
        "author": comment.author_name,
        "comment_excerpt": excerpt(&comment.content),
        "url": format!("{base}/admin/comments"),
    });
    vyasa_core::notify::EmailService::new(state.pool.clone())
        .queue(&author.email, "comment_notification", &ctx)
        .await?;
    state.metrics.inc_email_queued();
    state.publisher_notify.notify_one();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{excerpt, EXCERPT_CHARS};

    #[test]
    fn short_comments_are_quoted_whole() {
        assert_eq!(excerpt("  hello  "), "hello");
    }

    #[test]
    fn long_comments_are_cut_with_an_ellipsis() {
        let body = "a".repeat(EXCERPT_CHARS + 50);
        let out = excerpt(&body);
        assert_eq!(out.chars().count(), EXCERPT_CHARS + 1);
        assert!(out.ends_with('…'));
    }

    #[test]
    fn multibyte_comments_do_not_split_a_character() {
        // Slicing by bytes here would panic; taking chars cannot.
        let body = "é".repeat(EXCERPT_CHARS + 10);
        let out = excerpt(&body);
        assert!(out.starts_with('é'));
        assert_eq!(out.chars().count(), EXCERPT_CHARS + 1);
    }
}
