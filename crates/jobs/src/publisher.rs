//! Scheduled post publisher worker.

use std::time::Duration;

use sqlx::PgPool;
use tokio::sync::Notify;
use vyasa_core::post::PostService;
use vyasa_db::repo::PostsRepo;

/// How often the worker scans for due scheduled posts.
pub const POLL_INTERVAL: Duration = Duration::from_secs(30);

/// Publishes due scheduled posts once.
///
/// Returns the number of posts published.
///
/// # Errors
///
/// Returns [`vyasa_common::AppError::Db`] on database failure.
pub async fn publish_due_once(pool: &PgPool) -> Result<usize, vyasa_common::AppError> {
    let service = PostService::new(PostsRepo::new(pool.clone()));
    service.publish_due().await
}

/// Spawns the publisher loop. The returned handle and notifier allow
/// on-demand nudges after scheduling a post.
///
/// The loop runs every [`POLL_INTERVAL`] and also when `notify` is
/// triggered.
pub fn spawn(pool: PgPool, notify: std::sync::Arc<Notify>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(POLL_INTERVAL);
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    if let Err(err) = publish_due_once(&pool).await {
                        tracing::warn!("scheduled publisher tick failed: {err}");
                    }
                }
                () = notify.notified() => {
                    if let Err(err) = publish_due_once(&pool).await {
                        tracing::warn!("scheduled publisher nudge failed: {err}");
                    }
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use vyasa_core::block::{Block, BlockDocument, BlockKind};
    use vyasa_core::post::{CreatePost, PostService};
    use vyasa_db::content_models::{PostStatus, PostType};
    use vyasa_db::repo::PostsRepo;
    use vyasa_testkit::TestDb;

    fn block_doc() -> BlockDocument {
        BlockDocument::new(vec![Block {
            kind: BlockKind::Paragraph,
            plugin_kind: None,
            attrs: json!({"text": "hi"}),
            children: Vec::new(),
        }])
    }

    async fn setup() -> (TestDb, PostService, i64) {
        let db = TestDb::new().await;
        let pool = db.pool().clone();
        // need a user
        let hash = vyasa_core::user::password::hash_password("pw").expect("hash");
        let user_id = vyasa_common::next_id_i64();
        let email = format!("sched-{user_id}@example.com");
        let username = format!("sched{user_id}");
        sqlx::query(
            "INSERT INTO users (id, email, username, display_name, password_hash, role) VALUES ($1, $2, $3, $4, $5, 'author')",
        )
        .bind(user_id)
        .bind(email)
        .bind(username)
        .bind("Sched")
        .bind(hash)
        .execute(&pool)
        .await
        .expect("insert user");
        let service = PostService::new(PostsRepo::new(pool.clone()));
        (db, service, user_id)
    }

    #[tokio::test]
    #[allow(clippy::similar_names)]
    async fn publishes_past_due() {
        let (_db, service, user_id) = setup().await;
        // Create a scheduled post with past time.
        let past = chrono::Utc::now() - chrono::Duration::seconds(10);
        let created = service
            .create(CreatePost {
                post_type: PostType::Post,
                status: PostStatus::Scheduled,
                title: "Scheduled".into(),
                slug: None,
                content: block_doc(),
                excerpt: None,
                author_id: user_id,
                parent_id: None,
                scheduled_for: Some(past),
                password: None,
                term_ids: None,
                layout: None,
            })
            .await
            .expect("create scheduled");
        assert_eq!(created.status, PostStatus::Scheduled);
        let n = service.publish_due().await.expect("publish_due");
        assert_eq!(n, 1);
        let fetched = service.get(created.id).await.expect("get");
        assert_eq!(fetched.status, PostStatus::Published);
        assert!(fetched.published_at.is_some());
    }

    #[tokio::test]
    async fn does_not_publish_future() {
        let (_db, service, user_id) = setup().await;
        let future = chrono::Utc::now() + chrono::Duration::seconds(3600);
        let post = service
            .create(CreatePost {
                post_type: PostType::Post,
                status: PostStatus::Scheduled,
                title: "Future".into(),
                slug: None,
                content: block_doc(),
                excerpt: None,
                author_id: user_id,
                parent_id: None,
                scheduled_for: Some(future),
                password: None,
                term_ids: None,
                layout: None,
            })
            .await
            .expect("create");
        let n = service.publish_due().await.expect("publish_due");
        assert_eq!(n, 0);
        let fetched = service.get(post.id).await.expect("get");
        assert_eq!(fetched.status, PostStatus::Scheduled);
    }
}
