//! The post service on hierarchical types.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use vyasa_common::AppError;
use vyasa_core::block::BlockDocument;
use vyasa_core::post::hierarchy::Move;
use vyasa_core::post::{CreatePost, PostService, UpdatePost};
use vyasa_db::content_models::{PostStatus, PostType};
use vyasa_db::repo::PostsRepo;
use vyasa_testkit::TestDb;

async fn setup() -> (TestDb, PostService, PostType) {
    let db = TestDb::new().await;
    sqlx::query(
        "INSERT INTO users (id, email, username, display_name, password_hash, role)
         VALUES (1, 'h@example.com', 'h', 'H', 'x', 'admin')",
    )
    .execute(db.pool())
    .await
    .expect("user");
    let ty = PostType::register("guide-core").expect("register");
    PostType::set_traits("guide-core", true, None).expect("traits");
    let svc = PostService::new(PostsRepo::new(db.pool().clone()));
    (db, svc, ty)
}

fn create(ty: PostType, title: &str, parent: Option<i64>) -> CreatePost {
    CreatePost {
        post_type: ty,
        status: PostStatus::Published,
        title: title.to_owned(),
        slug: None,
        content: BlockDocument::new(Vec::new()),
        excerpt: None,
        author_id: 1,
        parent_id: parent,
        scheduled_for: None,
        password: None,
        term_ids: None,
        layout: None,
        position: None,
    }
}

#[tokio::test]
async fn children_get_paths_and_appended_positions() {
    let (_db, svc, ty) = setup().await;
    let guides = svc.create(create(ty, "Guides", None)).await.unwrap();
    let install = svc
        .create(create(ty, "Install", Some(guides.id)))
        .await
        .unwrap();
    let deploy = svc
        .create(create(ty, "Deploy", Some(guides.id)))
        .await
        .unwrap();
    assert_eq!(guides.path.as_deref(), Some("guides"));
    assert_eq!(install.path.as_deref(), Some("guides/install"));
    assert!(
        deploy.position > install.position,
        "appended after its siblings"
    );
}

#[tokio::test]
async fn a_parent_must_be_a_live_entry_of_the_same_type() {
    let (_db, svc, ty) = setup().await;
    let other = svc
        .create(create(PostType::Post, "A post", None))
        .await
        .unwrap();
    let err = svc
        .create(create(ty, "Child", Some(other.id)))
        .await
        .unwrap_err();
    assert!(matches!(err, AppError::Validation { .. }), "{err:?}");
    let err = svc
        .create(create(ty, "Orphan", Some(9_999_999)))
        .await
        .unwrap_err();
    assert!(
        matches!(err, AppError::Validation { .. } | AppError::NotFound { .. }),
        "{err:?}"
    );
}

#[tokio::test]
async fn renaming_a_slug_rewrites_the_paths_below_it() {
    let (_db, svc, ty) = setup().await;
    let guides = svc.create(create(ty, "Guides", None)).await.unwrap();
    let install = svc
        .create(create(ty, "Install", Some(guides.id)))
        .await
        .unwrap();
    let out = svc
        .update(
            guides.id,
            UpdatePost {
                slug: Some("handbook".into()),
                ..UpdatePost::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(out.path.as_deref(), Some("handbook"));
    assert_eq!(
        svc.get(install.id).await.unwrap().path.as_deref(),
        Some("handbook/install")
    );
}

#[tokio::test]
async fn moving_reparents_and_rewrites_the_subtree() {
    let (_db, svc, ty) = setup().await;
    let a = svc.create(create(ty, "A", None)).await.unwrap();
    let b = svc.create(create(ty, "B", None)).await.unwrap();
    let x = svc.create(create(ty, "X", Some(a.id))).await.unwrap();
    let y = svc.create(create(ty, "Y", Some(x.id))).await.unwrap();
    let outcome = svc
        .move_entries(
            ty,
            &[Move {
                id: x.id,
                parent_id: Some(b.id),
                position: 10,
            }],
        )
        .await
        .unwrap();
    assert_eq!(svc.get(y.id).await.unwrap().path.as_deref(), Some("b/x/y"));
    let mut changed: Vec<_> = outcome
        .changed_paths
        .iter()
        .map(|(id, old, new)| (*id, old.as_str(), new.as_str()))
        .collect();
    changed.sort_unstable();
    assert_eq!(changed, [(x.id, "a/x", "b/x"), (y.id, "a/x/y", "b/x/y")]);
}

#[tokio::test]
async fn a_loop_a_deep_tree_and_a_sibling_clash_are_refused_whole() {
    let (_db, svc, ty) = setup().await;
    let a = svc.create(create(ty, "A", None)).await.unwrap();
    let b = svc.create(create(ty, "B", Some(a.id))).await.unwrap();
    let err = svc
        .move_entries(
            ty,
            &[Move {
                id: a.id,
                parent_id: Some(b.id),
                position: 0,
            }],
        )
        .await
        .unwrap_err();
    assert!(
        matches!(&err, AppError::Validation { message, .. } if message.contains("ancestor")),
        "{err:?}"
    );
    // A sibling clash: two entries called X under different parents, then one moved beside the other.
    let p = svc.create(create(ty, "P", None)).await.unwrap();
    let q = svc.create(create(ty, "Q", None)).await.unwrap();
    let _x1 = svc.create(create(ty, "X", Some(p.id))).await.unwrap();
    let x2 = svc.create(create(ty, "X", Some(q.id))).await.unwrap();
    let err = svc
        .move_entries(
            ty,
            &[Move {
                id: x2.id,
                parent_id: Some(p.id),
                position: 5,
            }],
        )
        .await
        .unwrap_err();
    assert!(
        matches!(&err, AppError::Validation { message, .. } if message.contains("same parent")),
        "{err:?}"
    );
    // Nothing changed.
    assert_eq!(svc.get(x2.id).await.unwrap().path.as_deref(), Some("q/x"));
}

#[tokio::test]
async fn a_swap_in_one_request_succeeds() {
    let (_db, svc, ty) = setup().await;
    let a = svc.create(create(ty, "A", None)).await.unwrap();
    let b = svc.create(create(ty, "B", None)).await.unwrap();
    let xa = svc.create(create(ty, "X", Some(a.id))).await.unwrap();
    let xb = svc.create(create(ty, "X", Some(b.id))).await.unwrap();
    svc.move_entries(
        ty,
        &[
            Move {
                id: xa.id,
                parent_id: Some(b.id),
                position: 10,
            },
            Move {
                id: xb.id,
                parent_id: Some(a.id),
                position: 10,
            },
        ],
    )
    .await
    .unwrap();
    assert_eq!(svc.get(xa.id).await.unwrap().path.as_deref(), Some("b/x"));
    assert_eq!(svc.get(xb.id).await.unwrap().path.as_deref(), Some("a/x"));
}

#[tokio::test]
async fn the_root_slug_search_is_reserved() {
    let (_db, svc, ty) = setup().await;
    let err = svc.create(create(ty, "Search", None)).await.unwrap_err();
    assert!(
        matches!(&err, AppError::Validation { message, .. } if message.contains("reserved")),
        "{err:?}"
    );
    let docs = svc.create(create(ty, "Docs", None)).await.unwrap();
    svc.create(create(ty, "Search", Some(docs.id)))
        .await
        .expect("fine below the root");
}

#[tokio::test]
async fn rebuild_paths_recomputes_every_path_from_the_parent_links() {
    let (db, svc, ty) = setup().await;
    let a = svc.create(create(ty, "A", None)).await.unwrap();
    let x = svc.create(create(ty, "X", Some(a.id))).await.unwrap();
    // What an import leaves behind before the parents are known: throwaway paths.
    sqlx::query("UPDATE posts SET path = '~' || id WHERE type = 'guide-core'")
        .execute(db.pool())
        .await
        .unwrap();
    assert_eq!(svc.rebuild_paths(ty).await.unwrap(), 2);
    assert_eq!(svc.get(a.id).await.unwrap().path.as_deref(), Some("a"));
    assert_eq!(svc.get(x.id).await.unwrap().path.as_deref(), Some("a/x"));
}

#[tokio::test]
async fn a_type_that_is_not_hierarchical_refuses_to_move() {
    let (_db, svc, _ty) = setup().await;
    let p = svc.create(create(PostType::Post, "P", None)).await.unwrap();
    let err = svc
        .move_entries(
            PostType::Post,
            &[Move {
                id: p.id,
                parent_id: None,
                position: 1,
            }],
        )
        .await
        .unwrap_err();
    assert!(matches!(err, AppError::Validation { .. }), "{err:?}");
}

/// A group `a` with child `b` and grandchild `c`, then `a` trashed: `b`
/// and `c` are orphans stored at `a/b` and `a/b/c`.
async fn orphaned(svc: &PostService, ty: PostType) -> (i64, i64, i64) {
    let a = svc.create(create(ty, "A", None)).await.unwrap();
    let b = svc.create(create(ty, "B", Some(a.id))).await.unwrap();
    let c = svc.create(create(ty, "C", Some(b.id))).await.unwrap();
    svc.trash(a.id).await.unwrap();
    (a.id, b.id, c.id)
}

async fn path(svc: &PostService, id: i64) -> String {
    svc.get(id).await.unwrap().path.unwrap_or_default()
}

#[tokio::test]
async fn a_restore_under_a_trashed_parent_is_refused() {
    let (_db, svc, ty) = setup().await;
    let (a, b, _c) = orphaned(&svc, ty).await;
    svc.trash(b).await.unwrap();
    let err = svc.restore(b).await.unwrap_err();
    assert!(
        err.to_string().contains(&format!(
            "entry {b} has a parent ({a}) that is not in this type"
        )),
        "{err}"
    );
}

#[tokio::test]
async fn an_orphan_position_update_keeps_its_stored_path() {
    let (_db, svc, ty) = setup().await;
    let (a, b, c) = orphaned(&svc, ty).await;
    for id in [b, c] {
        let parent = svc.get(id).await.unwrap().parent_id;
        svc.update(
            id,
            UpdatePost {
                parent_id: Some(parent),
                position: Some(77),
                ..UpdatePost::default()
            },
        )
        .await
        .unwrap();
    }
    assert_eq!(svc.get(b).await.unwrap().parent_id, Some(a));
    assert_eq!(path(&svc, b).await, "a/b");
    assert_eq!(path(&svc, c).await, "a/b/c");
}

#[tokio::test]
async fn a_write_elsewhere_in_a_type_with_an_orphan_succeeds() {
    let (_db, svc, ty) = setup().await;
    orphaned(&svc, ty).await;
    let other = svc.create(create(ty, "Other", None)).await.unwrap();
    let out = svc
        .update(
            other.id,
            UpdatePost {
                slug: Some("elsewhere".into()),
                ..UpdatePost::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(out.path.as_deref(), Some("elsewhere"));
}

#[tokio::test]
async fn writes_that_recompute_an_orphan_subtree_keep_its_prefix() {
    let (_db, svc, ty) = setup().await;
    let (_a, b, c) = orphaned(&svc, ty).await;
    let live = svc.create(create(ty, "Live", None)).await.unwrap();
    // A reorder in the tree view (position only).
    let outcome = svc
        .move_entries(
            ty,
            &[Move {
                id: c,
                parent_id: Some(b),
                position: 99,
            }],
        )
        .await
        .unwrap();
    assert!(
        outcome.changed_paths.is_empty(),
        "{:?}",
        outcome.changed_paths
    );
    assert_eq!(path(&svc, c).await, "a/b/c");
    // A slug edit of the grandchild, then of the orphan itself.
    let slug = |s: &str| UpdatePost {
        slug: Some(s.into()),
        ..UpdatePost::default()
    };
    svc.update(c, slug("d")).await.unwrap();
    assert_eq!(path(&svc, c).await, "a/b/d");
    svc.update(b, slug("b2")).await.unwrap();
    assert_eq!(path(&svc, b).await, "a/b2");
    assert_eq!(path(&svc, c).await, "a/b2/d");
    // A live entry moved under the orphan joins its address.
    svc.move_entries(
        ty,
        &[Move {
            id: live.id,
            parent_id: Some(c),
            position: 0,
        }],
    )
    .await
    .unwrap();
    assert_eq!(path(&svc, live.id).await, "a/b2/d/live");
    // An import's rebuild rewrites nothing in the orphan subtree.
    svc.rebuild_paths(ty).await.unwrap();
    assert_eq!(path(&svc, b).await, "a/b2");
    assert_eq!(path(&svc, live.id).await, "a/b2/d/live");
    // Moving the orphan under the live root repairs it.
    svc.move_entries(
        ty,
        &[Move {
            id: b,
            parent_id: None,
            position: 0,
        }],
    )
    .await
    .unwrap();
    assert_eq!(path(&svc, b).await, "b2");
    assert_eq!(path(&svc, live.id).await, "b2/d/live");
}
