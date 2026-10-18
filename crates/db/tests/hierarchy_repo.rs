//! The repository half of hierarchical post types: sibling-unique slugs
//! through a materialised path, atomic subtree moves, path lookups.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use vyasa_common::AppError;
use vyasa_db::content_models::{PostStatus, PostType};
use vyasa_db::repo::posts::TreeChange;
use vyasa_db::repo::{NewPost, PostsRepo};
use vyasa_testkit::TestDb;

async fn seed_author(pool: &sqlx::PgPool) {
    sqlx::query(
        "INSERT INTO users (id, email, username, display_name, password_hash, role)
         VALUES (1, 'h@example.com', 'h', 'H', 'x', 'admin')",
    )
    .execute(pool)
    .await
    .expect("user");
}

fn guide() -> PostType {
    let t = PostType::register("guide-repo").expect("register");
    PostType::set_traits("guide-repo", true, None).expect("traits");
    t
}

fn new(id: i64, slug: &str, parent: Option<i64>, prefix: Option<&str>, ty: PostType) -> NewPost {
    NewPost {
        id,
        post_type: ty,
        status: PostStatus::Published,
        slug: slug.to_owned(),
        title: slug.to_owned(),
        content: serde_json::json!({"schema_version": 1, "blocks": []}),
        excerpt: None,
        author_id: 1,
        parent_id: parent,
        meta: serde_json::json!({}),
        published_at: None,
        scheduled_for: None,
        password_hash: None,
        layout: None,
        position: 0,
        path_prefix: prefix.map(str::to_owned),
    }
}

#[tokio::test]
async fn type_traits_say_whether_a_type_nests_and_where_it_lives() {
    let t = PostType::register("traits-probe").expect("register");
    assert!(!t.is_hierarchical());
    assert_eq!(t.base(), "traits-probe");
    PostType::set_traits("traits-probe", true, Some("probe-docs")).expect("traits");
    assert!(t.is_hierarchical());
    assert_eq!(t.base(), "probe-docs");
    assert_eq!(PostType::find_by_base("probe-docs"), Some(t));
    assert_eq!(
        PostType::find_by_base("traits-probe"),
        None,
        "the base replaces the slug as the address"
    );
    PostType::clear_traits("traits-probe");
    assert!(!t.is_hierarchical());
    assert_eq!(PostType::find_by_base("traits-probe"), Some(t));
    // Built-ins are their own base and never hierarchical.
    assert!(!PostType::Post.is_hierarchical());
    assert_eq!(PostType::Page.base(), "page");
    assert!(PostType::set_traits("post", true, None).is_err());
    assert!(PostType::set_traits("Bad Base", false, Some("x y")).is_err());
}

#[tokio::test]
async fn cousins_may_share_a_slug_but_siblings_get_a_suffix() {
    let db = TestDb::new().await;
    seed_author(db.pool()).await;
    let repo = PostsRepo::new(db.pool().clone());
    let ty = guide();
    let a = repo
        .insert_with_unique_slug(&new(10, "guides", None, Some(""), ty), None)
        .await
        .unwrap();
    let b = repo
        .insert_with_unique_slug(&new(11, "api", None, Some(""), ty), None)
        .await
        .unwrap();
    assert_eq!(
        (a.path.as_deref(), b.path.as_deref()),
        (Some("guides"), Some("api"))
    );
    let x1 = repo
        .insert_with_unique_slug(&new(12, "install", Some(10), Some("guides/"), ty), None)
        .await
        .unwrap();
    let x2 = repo
        .insert_with_unique_slug(&new(13, "install", Some(11), Some("api/"), ty), None)
        .await
        .unwrap();
    assert_eq!(x1.slug, "install");
    assert_eq!(x2.slug, "install");
    assert_eq!(x2.path.as_deref(), Some("api/install"));
    let x3 = repo
        .insert_with_unique_slug(&new(14, "install", Some(10), Some("guides/"), ty), None)
        .await
        .unwrap();
    assert_eq!(x3.slug, "install-2");
    assert_eq!(x3.path.as_deref(), Some("guides/install-2"));
}

#[tokio::test]
async fn non_hierarchical_rows_keep_the_old_rule() {
    let db = TestDb::new().await;
    seed_author(db.pool()).await;
    let repo = PostsRepo::new(db.pool().clone());
    repo.insert(&new(1, "hello", None, None, PostType::Post))
        .await
        .unwrap();
    let again = repo
        .insert(&new(2, "hello", None, None, PostType::Post))
        .await;
    assert!(matches!(again, Err(AppError::Conflict { .. })), "{again:?}");
    // A trashed row frees the slug, as before.
    sqlx::query("UPDATE posts SET status = 'trash' WHERE id = 1")
        .execute(db.pool())
        .await
        .unwrap();
    repo.insert(&new(3, "hello", None, None, PostType::Post))
        .await
        .unwrap();
    let row = repo.get(3).await.unwrap();
    assert_eq!((row.position, row.path), (0, None));
}

#[tokio::test]
async fn apply_tree_can_swap_two_entries() {
    let db = TestDb::new().await;
    seed_author(db.pool()).await;
    let repo = PostsRepo::new(db.pool().clone());
    let ty = guide();
    for (id, slug, parent, prefix) in [
        (20, "a", None, ""),
        (21, "b", None, ""),
        (22, "x", Some(20), "a/"),
        (23, "x", Some(21), "b/"),
    ] {
        repo.insert_with_unique_slug(&new(id, slug, parent, Some(prefix), ty), None)
            .await
            .unwrap();
    }
    // x moves from a to b and the other x from b to a: for an instant both
    // would be `b/x`, so the repository must not write them one by one.
    repo.apply_tree(
        ty,
        &[
            TreeChange {
                id: 22,
                parent_id: Some(21),
                position: 10,
                path: "b/x".into(),
            },
            TreeChange {
                id: 23,
                parent_id: Some(20),
                position: 10,
                path: "a/x".into(),
            },
        ],
    )
    .await
    .unwrap();
    assert_eq!(repo.get(22).await.unwrap().path.as_deref(), Some("b/x"));
    assert_eq!(repo.get(23).await.unwrap().path.as_deref(), Some("a/x"));
    assert_eq!(repo.get(23).await.unwrap().parent_id, Some(20));
}

#[tokio::test]
async fn apply_tree_refuses_a_collision_and_changes_nothing() {
    let db = TestDb::new().await;
    seed_author(db.pool()).await;
    let repo = PostsRepo::new(db.pool().clone());
    let ty = guide();
    repo.insert_with_unique_slug(&new(30, "a", None, Some(""), ty), None)
        .await
        .unwrap();
    repo.insert_with_unique_slug(&new(31, "b", None, Some(""), ty), None)
        .await
        .unwrap();
    let err = repo
        .apply_tree(
            ty,
            &[
                TreeChange {
                    id: 30,
                    parent_id: None,
                    position: 99,
                    path: "taken".into(),
                },
                TreeChange {
                    id: 31,
                    parent_id: None,
                    position: 5,
                    path: "taken".into(),
                },
            ],
        )
        .await;
    assert!(matches!(err, Err(AppError::Conflict { .. })), "{err:?}");
    let a = repo.get(30).await.unwrap();
    assert_eq!(
        (a.path.as_deref(), a.position),
        (Some("a"), 0),
        "rolled back"
    );
}

#[tokio::test]
async fn lookups_by_path_rows_in_order_and_the_first_root() {
    let db = TestDb::new().await;
    seed_author(db.pool()).await;
    let repo = PostsRepo::new(db.pool().clone());
    let ty = guide();
    let mut p = new(40, "later", None, Some(""), ty);
    p.position = 20;
    repo.insert_with_unique_slug(&p, None).await.unwrap();
    let mut p = new(41, "first", None, Some(""), ty);
    p.position = 10;
    repo.insert_with_unique_slug(&p, None).await.unwrap();
    repo.insert_with_unique_slug(&new(42, "child", Some(41), Some("first/"), ty), None)
        .await
        .unwrap();
    assert_eq!(repo.get_by_path(ty, "first/child").await.unwrap().id, 42);
    assert!(repo.get_by_path(ty, "nope").await.is_err());
    assert_eq!(repo.first_root(ty).await.unwrap().map(|r| r.id), Some(41));
    sqlx::query("UPDATE posts SET status = 'trash' WHERE id = 42")
        .execute(db.pool())
        .await
        .unwrap();
    let rows = repo.tree_rows(ty).await.unwrap();
    assert_eq!(
        rows.iter().map(|r| r.id).collect::<Vec<_>>(),
        [41, 40],
        "trash excluded, ordered by position"
    );
    assert!(
        repo.get_by_path(ty, "first/child").await.is_err(),
        "a trashed entry has no address"
    );
}

#[tokio::test]
async fn switching_a_type_to_hierarchical_backfills_flat_paths_and_back() {
    let db = TestDb::new().await;
    seed_author(db.pool()).await;
    let repo = PostsRepo::new(db.pool().clone());
    let ty = PostType::register("late-guide").expect("register");
    repo.insert(&new(50, "one", None, None, ty)).await.unwrap();
    repo.insert(&new(51, "two", None, None, ty)).await.unwrap();
    assert_eq!(repo.backfill_paths(ty).await.unwrap(), 2);
    assert_eq!(repo.get(50).await.unwrap().path.as_deref(), Some("one"));
    assert_eq!(repo.clear_paths(ty).await.unwrap(), 2);
    assert_eq!(repo.get(51).await.unwrap().path, None);
}
