//! Newest and Oldest go by publication time, not by when a row was created.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use chrono::{TimeZone, Utc};
use vyasa_db::content_models::{PostStatus, PostType};
use vyasa_db::repo::posts::{PostFilter, PostSort};
use vyasa_db::repo::{NewPost, PostsRepo};
use vyasa_testkit::TestDb;

fn published(id: i64, slug: &str, day: u32) -> NewPost {
    NewPost {
        id,
        post_type: PostType::Post,
        status: PostStatus::Published,
        slug: slug.to_owned(),
        title: slug.to_owned(),
        content: serde_json::json!({"schema_version": 1, "blocks": []}),
        excerpt: None,
        author_id: 1,
        parent_id: None,
        meta: serde_json::json!({}),
        published_at: Some(Utc.with_ymd_and_hms(2026, 11, day, 9, 0, 0).unwrap()),
        scheduled_for: None,
        password_hash: None,
        layout: None,
        position: 0,
        path_prefix: None,
    }
}

#[tokio::test]
async fn newest_and_oldest_follow_publication_time_not_creation_time() {
    let db = TestDb::new().await;
    sqlx::query(
        "INSERT INTO users (id, email, username, display_name, password_hash, role)
         VALUES (1, 's@example.com', 's', 'S', 'x', 'admin')",
    )
    .execute(db.pool())
    .await
    .expect("user");
    let repo = PostsRepo::new(db.pool().clone());
    // Created in the order a, b, c; published on the 18th, 14th and 15th.
    for p in [
        published(1, "a-18th", 18),
        published(2, "b-14th", 14),
        published(3, "c-15th", 15),
    ] {
        repo.insert(&p).await.expect("insert");
    }
    let newest = PostFilter {
        sort: PostSort::Newest,
        limit: 10,
        ..PostFilter::default()
    };
    assert_eq!(
        repo.list(&newest)
            .await
            .unwrap()
            .into_iter()
            .map(|r| r.slug)
            .collect::<Vec<_>>(),
        ["a-18th", "c-15th", "b-14th"]
    );
    let oldest = PostFilter {
        sort: PostSort::Oldest,
        limit: 10,
        ..PostFilter::default()
    };
    assert_eq!(
        repo.list(&oldest)
            .await
            .unwrap()
            .into_iter()
            .map(|r| r.slug)
            .collect::<Vec<_>>(),
        ["b-14th", "c-15th", "a-18th"]
    );
}
