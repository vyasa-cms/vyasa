//! The slug probes behind `PostsRepo::insert_with_unique_slug` run under
//! a per-base advisory lock on every post create, so they must be index
//! lookups on the partial `(type, slug)` index, never a scan of every
//! post of the type.
//!
//! The planner rightly prefers a scan on a small table (measured on a
//! scratch table: the candidates probe switches to the index somewhere
//! between 50k and 200k posts of one type, and costs a few ms either
//! way), so the test loads 200k posts, the size the old prefix probe was
//! measured at (a 45-50 ms sequential scan under the lock), and checks
//! the natural plans: both probes put `slug` in an index condition on the
//! partial index, and neither scans the table.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use vyasa_db::content_models::{PostStatus, PostType};
use vyasa_db::repo::posts::{BASE_SLUG_TAKEN_SQL, SLUG_INDEX, TAKEN_SLUGS_SQL};
use vyasa_db::repo::{NewPost, PostsRepo};
use vyasa_testkit::TestDb;

async fn seed_author(pool: &sqlx::PgPool) {
    sqlx::query(
        "INSERT INTO users (id, email, username, display_name, password_hash, role)
         VALUES (1, 'slug@example.com', 'slug', 'Slug', 'x', 'admin')",
    )
    .execute(pool)
    .await
    .expect("user");
}

async fn plan(pool: &sqlx::PgPool, sql: &str, slug_arg: SlugArg) -> String {
    let query = format!("EXPLAIN {sql}");
    let q = sqlx::query_scalar::<_, String>(&query).bind("post");
    let q = match slug_arg {
        SlugArg::One(slug) => q.bind(slug),
        SlugArg::Many(slugs) => q.bind(slugs),
    };
    q.fetch_all(pool).await.expect("explain").join("\n")
}

enum SlugArg {
    One(String),
    Many(Vec<String>),
}

#[tokio::test]
async fn slug_probes_use_the_partial_slug_index() {
    let db = TestDb::new().await;
    let pool = db.pool();
    seed_author(pool).await;
    sqlx::query(
        "INSERT INTO posts (id, type, status, slug, title, author_id)
         SELECT n, 'post', 'draft', 'post-' || n, 'T', 1
         FROM generate_series(1, 200000) AS n",
    )
    .execute(pool)
    .await
    .expect("posts");
    sqlx::query("ANALYZE posts")
        .execute(pool)
        .await
        .expect("analyze");

    let one = plan(pool, BASE_SLUG_TAKEN_SQL, SlugArg::One("post".into())).await;
    let candidates: Vec<String> = vyasa_common::slug_candidates("post").collect();
    let many = plan(pool, TAKEN_SLUGS_SQL, SlugArg::Many(candidates)).await;
    for (name, plan) in [("base probe", &one), ("candidates probe", &many)] {
        assert!(
            plan.contains(SLUG_INDEX),
            "{name} misses the index:\n{plan}"
        );
        let slug_is_index_cond = plan
            .lines()
            .any(|line| line.contains("Index Cond") && line.contains("slug"));
        assert!(
            slug_is_index_cond,
            "{name} filters slugs row by row:\n{plan}"
        );
        assert!(!plan.contains("Seq Scan"), "{name} scans:\n{plan}");
    }
}

/// With the base and every numbered suffix taken, the slug falls back to
/// the post's own snowflake id, which no other post can be minting.
#[tokio::test]
async fn exhausted_suffixes_fall_back_to_the_post_id() {
    let db = TestDb::new().await;
    let pool = db.pool();
    seed_author(pool).await;
    sqlx::query(
        "INSERT INTO posts (id, type, status, slug, title, author_id)
         SELECT 1, 'post', 'draft', 'full', 'T', 1
         UNION ALL
         SELECT n, 'post', 'draft', 'full-' || n, 'T', 1
         FROM generate_series(2, 999) AS n",
    )
    .execute(pool)
    .await
    .expect("posts");

    let id = 424_242;
    let row = PostsRepo::new(pool.clone())
        .insert_with_unique_slug(
            &NewPost {
                id,
                post_type: PostType::Post,
                status: PostStatus::Draft,
                slug: "full".into(),
                title: "Full".into(),
                content: serde_json::json!([]),
                excerpt: None,
                author_id: 1,
                parent_id: None,
                meta: serde_json::json!({}),
                published_at: None,
                scheduled_for: None,
                password_hash: None,
                layout: None,
            },
            None,
        )
        .await
        .expect("insert");
    assert_eq!(row.slug, format!("full-{id}"));
}
