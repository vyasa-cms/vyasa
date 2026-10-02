//! Integrity rules for posts, revisions, terms and user deletion: the
//! places where a check and a write used to be separate statements.
//!
//! Every test makes its own users and slugs rather than wiping tables.

#![allow(
    clippy::pedantic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_lines
)]

use serde_json::json;
use vyasa_common::AppError;
use vyasa_core::block::{Block, BlockDocument, BlockKind};
use vyasa_core::events::{self, Event};
use vyasa_core::post::{CreatePost, PostService, RevisionService, Snapshot, UpdatePost};
use vyasa_core::taxonomy::TermService;
use vyasa_core::user::UsersService;
use vyasa_db::content_models::{PostRow, PostStatus, PostType, Taxonomy};
use vyasa_db::models::Role;
use vyasa_db::repo::{NewUser, PostsRepo, RevisionsRepo, TermsRepo, UsersRepo};
use vyasa_testkit::TestDb;

struct Ctx {
    _db: TestDb,
    pool: sqlx::PgPool,
    posts: PostService,
}

async fn setup() -> Ctx {
    let db = TestDb::new().await;
    let pool = db.pool().clone();
    let posts = PostService::new(PostsRepo::new(pool.clone()));
    Ctx {
        _db: db,
        pool,
        posts,
    }
}

async fn user(pool: &sqlx::PgPool, role: Role) -> i64 {
    let id = vyasa_common::next_id_i64();
    UsersRepo::new(pool.clone())
        .insert(&NewUser {
            id,
            email: &format!("pi{id}@example.com"),
            username: &format!("pi{id}"),
            display_name: "PI",
            password_hash: Some("x"),
            role,
            bio: "",
        })
        .await
        .expect("user");
    id
}

fn doc(text: &str) -> BlockDocument {
    BlockDocument::new(vec![Block {
        kind: BlockKind::Paragraph,
        plugin_kind: None,
        attrs: json!({ "text": text }),
        children: Vec::new(),
    }])
}

fn uniq(prefix: &str) -> String {
    format!("{prefix}-{}", vyasa_common::next_id_i64())
}

fn new_post(author: i64, title: &str, slug: Option<String>) -> CreatePost {
    CreatePost {
        post_type: PostType::Post,
        status: PostStatus::Draft,
        title: title.into(),
        slug,
        content: doc("hello"),
        excerpt: None,
        author_id: author,
        parent_id: None,
        scheduled_for: None,
        password: None,
        term_ids: None,
        layout: None,
    }
}

/// Events for `post_id` that arrived on `rx` so far.
fn drain(rx: &mut tokio::sync::broadcast::Receiver<Event>, post_id: i64) -> Vec<&'static str> {
    let mut out = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        let (kind, id) = match ev {
            Event::Published(e) => ("published", e.post_id),
            Event::Updated(e) => ("updated", e.post_id),
            Event::Trashed(e) => ("trashed", e.post_id),
            Event::Restored(e) => ("restored", e.post_id),
            Event::Deleted(e) => ("deleted", e.post_id),
            _ => continue,
        };
        if id == post_id {
            out.push(kind);
        }
    }
    out
}

async fn post_row(pool: &sqlx::PgPool, id: i64) -> PostRow {
    PostsRepo::new(pool.clone()).get(id).await.expect("post")
}

#[tokio::test]
async fn a_revision_restores_only_into_its_own_post_and_announces_it() {
    let ctx = setup().await;
    let author = user(&ctx.pool, Role::Author).await;
    let revs = RevisionService::new(
        RevisionsRepo::new(ctx.pool.clone()),
        PostsRepo::new(ctx.pool.clone()),
    );
    let mine = ctx
        .posts
        .create(new_post(author, "Mine", None))
        .await
        .unwrap();
    let theirs = ctx
        .posts
        .create(new_post(author, "Theirs", None))
        .await
        .unwrap();
    let their_rev = revs
        .snapshot_if_changed(theirs.id, author, &Snapshot::of(&theirs))
        .await
        .unwrap()
        .expect("revision");

    let err = revs
        .restore(mine.id, their_rev.id, author)
        .await
        .expect_err("foreign revision");
    assert!(matches!(err, AppError::NotFound { .. }), "{err:?}");
    assert_eq!(post_row(&ctx.pool, mine.id).await.title, "Mine");

    let mut rx = events::subscribe();
    let restored = revs.restore(theirs.id, their_rev.id, author).await.unwrap();
    assert_eq!(restored.id, theirs.id);
    assert_eq!(drain(&mut rx, theirs.id), vec!["updated"]);
}

#[tokio::test]
async fn unscheduling_drops_the_date() {
    let ctx = setup().await;
    let author = user(&ctx.pool, Role::Author).await;
    let post = ctx
        .posts
        .create(new_post(author, "Sched", None))
        .await
        .unwrap();
    let later = chrono::Utc::now() + chrono::Duration::hours(2);
    ctx.posts
        .update(
            post.id,
            UpdatePost {
                status: Some(PostStatus::Scheduled),
                scheduled_for: Some(later),
                ..UpdatePost::default()
            },
        )
        .await
        .unwrap();
    let draft = ctx
        .posts
        .update(
            post.id,
            UpdatePost {
                status: Some(PostStatus::Draft),
                clear_schedule: true,
                ..UpdatePost::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(draft.scheduled_for, None, "the schedule was cleared");
    let err = ctx
        .posts
        .update(
            post.id,
            UpdatePost {
                status: Some(PostStatus::Scheduled),
                ..UpdatePost::default()
            },
        )
        .await
        .expect_err("no date");
    assert!(matches!(err, AppError::Validation { .. }), "{err:?}");
}

#[tokio::test]
async fn due_posts_publish_exactly_once_under_concurrent_publishers() {
    let ctx = setup().await;
    let author = user(&ctx.pool, Role::Author).await;
    let past = chrono::Utc::now() - chrono::Duration::seconds(5);
    let mut input = new_post(author, "Due", None);
    input.status = PostStatus::Scheduled;
    input.scheduled_for = Some(past);
    let due = ctx.posts.create(input).await.unwrap();
    // A post that was moved back to draft keeps no claim on publishing,
    // even with a stale date still on the row.
    let mut input = new_post(author, "Pulled", None);
    input.status = PostStatus::Scheduled;
    input.scheduled_for = Some(past);
    let pulled = ctx.posts.create(input).await.unwrap();
    sqlx::query("UPDATE posts SET status = 'draft' WHERE id = $1")
        .bind(pulled.id)
        .execute(&ctx.pool)
        .await
        .unwrap();

    let mut rx = events::subscribe();
    let (a, b, c) = tokio::join!(
        ctx.posts.publish_due(),
        ctx.posts.publish_due(),
        ctx.posts.publish_due()
    );
    a.unwrap();
    b.unwrap();
    c.unwrap();
    assert_eq!(drain(&mut rx, due.id), vec!["published"]);
    assert_eq!(
        post_row(&ctx.pool, due.id).await.status,
        PostStatus::Published
    );
    assert_eq!(drain(&mut rx, pulled.id), Vec::<&str>::new());
    assert_eq!(
        post_row(&ctx.pool, pulled.id).await.status,
        PostStatus::Draft
    );
}

#[tokio::test]
async fn an_unknown_term_leaves_no_partial_write() {
    let ctx = setup().await;
    let author = user(&ctx.pool, Role::Author).await;
    let slug = uniq("orphan");
    let mut input = new_post(author, "Orphan", Some(slug.clone()));
    input.term_ids = Some(vec![-42]);
    assert!(ctx.posts.create(input).await.is_err());
    let exists: Option<i64> = sqlx::query_scalar("SELECT id FROM posts WHERE slug = $1")
        .bind(&slug)
        .fetch_optional(&ctx.pool)
        .await
        .unwrap();
    assert_eq!(exists, None, "the post was not left behind");

    let post = ctx
        .posts
        .create(new_post(author, "Kept", None))
        .await
        .unwrap();
    let err = ctx
        .posts
        .update(
            post.id,
            UpdatePost {
                title: Some("Changed".into()),
                password: Some("long-enough-pw".into()),
                term_ids: Some(vec![-42]),
                ..UpdatePost::default()
            },
        )
        .await;
    assert!(err.is_err());
    let row = post_row(&ctx.pool, post.id).await;
    assert_eq!(row.title, "Kept");
    assert!(row.password_hash.is_none());
}

#[tokio::test]
async fn concurrent_creates_with_one_title_all_get_a_slug() {
    let ctx = setup().await;
    let author = user(&ctx.pool, Role::Author).await;
    let title = uniq("Race");
    let futs = (0..6).map(|_| ctx.posts.create(new_post(author, &title, None)));
    let rows = futures::future::join_all(futs).await;
    let mut slugs: Vec<String> = rows.into_iter().map(|r| r.expect("create").slug).collect();
    slugs.sort();
    slugs.dedup();
    assert_eq!(slugs.len(), 6);
}

/// Far more racers than the create path has retries: only serialising
/// the slug probe with the insert lets every one of them through.
#[tokio::test]
async fn many_concurrent_creates_with_one_title_all_succeed() {
    let ctx = setup().await;
    let author = user(&ctx.pool, Role::Author).await;
    let title = uniq("Stampede");
    let futs = (0..24).map(|_| ctx.posts.create(new_post(author, &title, None)));
    let rows = futures::future::join_all(futs).await;
    let mut slugs: Vec<String> = rows.into_iter().map(|r| r.expect("create").slug).collect();
    slugs.sort();
    slugs.dedup();
    assert_eq!(slugs.len(), 24);
}

#[tokio::test]
async fn slug_collisions_are_conflicts_not_db_errors() {
    let ctx = setup().await;
    let author = user(&ctx.pool, Role::Author).await;
    let taken = uniq("taken");
    ctx.posts
        .create(new_post(author, "A", Some(taken.clone())))
        .await
        .unwrap();
    let other = ctx.posts.create(new_post(author, "B", None)).await.unwrap();
    let err = ctx
        .posts
        .update(
            other.id,
            UpdatePost {
                slug: Some(taken.clone()),
                ..UpdatePost::default()
            },
        )
        .await
        .expect_err("taken");
    assert!(matches!(err, AppError::Conflict { .. }), "{err:?}");

    // trash -> draft through update() re-checks the slug as restore() does.
    let shared = uniq("shared");
    let first = ctx
        .posts
        .create(new_post(author, "First", Some(shared.clone())))
        .await
        .unwrap();
    ctx.posts.trash(first.id).await.unwrap();
    ctx.posts
        .create(new_post(author, "Second", Some(shared.clone())))
        .await
        .unwrap();
    let err = ctx
        .posts
        .update(
            first.id,
            UpdatePost {
                status: Some(PostStatus::Draft),
                ..UpdatePost::default()
            },
        )
        .await
        .expect_err("slug reclaimed");
    assert!(matches!(err, AppError::Conflict { .. }), "{err:?}");
}

#[tokio::test]
async fn trash_and_restore_announce_once() {
    let ctx = setup().await;
    let author = user(&ctx.pool, Role::Author).await;
    let post = ctx
        .posts
        .create(new_post(author, "Once", None))
        .await
        .unwrap();
    let mut rx = events::subscribe();
    ctx.posts.trash(post.id).await.unwrap();
    assert_eq!(drain(&mut rx, post.id), vec!["trashed"]);
    ctx.posts.restore(post.id).await.unwrap();
    assert_eq!(drain(&mut rx, post.id), vec!["restored"]);
}

#[tokio::test]
async fn term_hierarchy_never_cycles() {
    let ctx = setup().await;
    let terms = TermService::new(
        TermsRepo::new(ctx.pool.clone()),
        PostsRepo::new(ctx.pool.clone()),
    );
    let a = terms
        .create(Taxonomy::Category, &uniq("a"), None, None, None)
        .await
        .unwrap();
    let b = terms
        .create(Taxonomy::Category, &uniq("b"), None, Some(a.id), None)
        .await
        .unwrap();
    let c = terms
        .create(Taxonomy::Category, &uniq("c"), None, Some(b.id), None)
        .await
        .unwrap();
    // Merging a term into its own descendant would hand the descendant's
    // ancestors to it as children.
    let err = terms.merge(a.id, c.id).await.expect_err("descendant");
    assert!(matches!(err, AppError::Validation { .. }), "{err:?}");
    let err = terms
        .delete(a.id, Some(c.id))
        .await
        .expect_err("descendant");
    assert!(matches!(err, AppError::Validation { .. }), "{err:?}");
    assert!(terms.get(a.id).await.is_ok());

    // A cycle written behind the service's back does not hang lookups.
    sqlx::query("UPDATE terms SET parent_id = $1 WHERE id = $2")
        .bind(c.id)
        .bind(a.id)
        .execute(&ctx.pool)
        .await
        .unwrap();
    let ancestors = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        TermsRepo::new(ctx.pool.clone()).ancestors(c.id),
    )
    .await
    .expect("terminates")
    .unwrap();
    assert!(ancestors.contains(&a.id) && ancestors.contains(&b.id));
    sqlx::query("UPDATE terms SET parent_id = NULL WHERE id = $1")
        .bind(a.id)
        .execute(&ctx.pool)
        .await
        .unwrap();

    // A rejected update writes nothing, the parent included.
    let other = terms
        .create(Taxonomy::Category, &uniq("o"), None, None, None)
        .await
        .unwrap();
    let err = terms
        .update(c.id, None, Some(&other.slug), Some(Some(a.id)), None)
        .await
        .expect_err("slug taken");
    assert!(matches!(err, AppError::Conflict { .. }), "{err:?}");
    assert_eq!(terms.get(c.id).await.unwrap().parent_id, Some(b.id));
}

#[tokio::test]
async fn deleting_an_author_requires_a_new_owner_for_their_content() {
    let ctx = setup().await;
    let users = UsersService::new(UsersRepo::new(ctx.pool.clone()));
    let author = user(&ctx.pool, Role::Author).await;
    let heir = user(&ctx.pool, Role::Editor).await;
    let editor = user(&ctx.pool, Role::Editor).await;
    let post = ctx
        .posts
        .create(new_post(author, "Legacy", None))
        .await
        .unwrap();
    // The editor touched the author's post: a revision in their name.
    let revs = RevisionService::new(
        RevisionsRepo::new(ctx.pool.clone()),
        PostsRepo::new(ctx.pool.clone()),
    );
    revs.snapshot_if_changed(post.id, editor, &Snapshot::of(&post))
        .await
        .unwrap();

    let err = users.delete(author).await.expect_err("owns content");
    assert!(matches!(err, AppError::Conflict { .. }), "{err:?}");
    assert!(users.get(author).await.is_ok());

    let err = users
        .delete_reassigning(author, Some(author))
        .await
        .expect_err("to self");
    assert!(matches!(err, AppError::Validation { .. }), "{err:?}");

    users.delete_reassigning(author, Some(heir)).await.unwrap();
    assert!(users.get(author).await.is_err());
    assert_eq!(post_row(&ctx.pool, post.id).await.author_id, heir);

    // A user with only revisions on others' posts can go without a target:
    // those revisions pass to the post's author.
    users.delete(editor).await.unwrap();
    let owners: Vec<i64> =
        sqlx::query_scalar("SELECT author_id FROM post_revisions WHERE post_id = $1")
            .bind(post.id)
            .fetch_all(&ctx.pool)
            .await
            .unwrap();
    assert!(owners.iter().all(|o| *o == heir), "{owners:?}");
}

#[tokio::test]
async fn a_suspended_admin_does_not_count_toward_the_last_admin() {
    let ctx = setup().await;
    let repo = UsersRepo::new(ctx.pool.clone());
    let users = UsersService::new(repo.clone());
    // This test's database is its own and starts with no users, so this
    // normally suspends nobody; it stays as a guard in case the template
    // ever seeds an admin, which would otherwise count as the "other".
    let others: Vec<i64> = sqlx::query_scalar(
        "UPDATE users SET suspended_at = now()
         WHERE role = 'admin' AND suspended_at IS NULL RETURNING id",
    )
    .fetch_all(&ctx.pool)
    .await
    .unwrap();
    let active = user(&ctx.pool, Role::Admin).await;
    let suspended = user(&ctx.pool, Role::Admin).await;
    repo.set_suspended(suspended, true).await.unwrap();
    assert_eq!(repo.count_by_role(Role::Admin).await.unwrap(), 1);
    let del = users.delete(active).await;
    let demote = users.set_role(active, Role::Editor).await;
    // Put the world back before asserting.
    sqlx::query("UPDATE users SET suspended_at = NULL WHERE id = ANY($1)")
        .bind(&others)
        .execute(&ctx.pool)
        .await
        .unwrap();
    assert!(matches!(del, Err(AppError::Validation { .. })), "{del:?}");
    assert!(
        matches!(demote, Err(AppError::Validation { .. })),
        "{demote:?}"
    );
    // The suspended admin can go; the active one stays.
    users.delete(suspended).await.unwrap();
}

#[tokio::test]
async fn updated_at_moves_only_for_edits() {
    let ctx = setup().await;
    let author = user(&ctx.pool, Role::Author).await;
    let repo = PostsRepo::new(ctx.pool.clone());
    let post = ctx
        .posts
        .create(new_post(author, "Stamp", None))
        .await
        .unwrap();
    let old = chrono::Utc::now() - chrono::Duration::days(3);
    sqlx::query("UPDATE posts SET updated_at = $2 WHERE id = $1")
        .bind(post.id)
        .bind(old)
        .execute(&ctx.pool)
        .await
        .unwrap();
    let stamp = |row: &PostRow| row.updated_at.timestamp_micros();
    let base = stamp(&post_row(&ctx.pool, post.id).await);
    assert_eq!(base, old.timestamp_micros(), "an explicit value is kept");

    repo.set_sticky(post.id, true).await.unwrap();
    repo.set_language(post.id, "fr", None).await.unwrap();
    ctx.posts
        .update(
            post.id,
            UpdatePost {
                password: Some("long-enough-pw".into()),
                ..UpdatePost::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(stamp(&post_row(&ctx.pool, post.id).await), base);

    ctx.posts
        .update(
            post.id,
            UpdatePost {
                title: Some("Stamp 2".into()),
                ..UpdatePost::default()
            },
        )
        .await
        .unwrap();
    assert!(stamp(&post_row(&ctx.pool, post.id).await) > base);
}

async fn active_admins(pool: &sqlx::PgPool) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE role = 'admin' AND suspended_at IS NULL")
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn the_only_active_admin_cannot_be_suspended() {
    let ctx = setup().await;
    let users = UsersService::new(UsersRepo::new(ctx.pool.clone()));
    let only = user(&ctx.pool, Role::Admin).await;
    let editor = user(&ctx.pool, Role::Editor).await;

    let err = users.set_suspended(only, true).await.expect_err("last");
    assert!(matches!(err, AppError::Validation { .. }), "{err:?}");
    assert_eq!(active_admins(&ctx.pool).await, 1);

    // Anyone else can be, and a missing user is not found.
    users.set_suspended(editor, true).await.unwrap();
    assert!(users.get(editor).await.unwrap().suspended_at.is_some());
    let err = users.set_suspended(1, true).await.expect_err("missing");
    assert!(matches!(err, AppError::NotFound { .. }), "{err:?}");

    // With a second admin, the first can go; then the second is the last.
    let second = user(&ctx.pool, Role::Admin).await;
    users.set_suspended(only, true).await.unwrap();
    let err = users.set_suspended(second, true).await.expect_err("last");
    assert!(matches!(err, AppError::Validation { .. }), "{err:?}");
    // Suspending the already-suspended admin again changes nothing.
    users.set_suspended(only, true).await.unwrap();
    // Reinstating takes no guard.
    users.set_suspended(only, false).await.unwrap();
    users.set_suspended(editor, false).await.unwrap();
    assert_eq!(active_admins(&ctx.pool).await, 2);
}

#[tokio::test]
async fn two_admins_suspending_each_other_leave_exactly_one() {
    let ctx = setup().await;
    let users = UsersService::new(UsersRepo::new(ctx.pool.clone()));
    let a = user(&ctx.pool, Role::Admin).await;
    let b = user(&ctx.pool, Role::Admin).await;

    for round in 0..20 {
        let (one, two) = (users.clone(), users.clone());
        let first = tokio::spawn(async move { one.set_suspended(a, true).await });
        let second = tokio::spawn(async move { two.set_suspended(b, true).await });
        let results = [first.await.unwrap(), second.await.unwrap()];
        assert_eq!(
            active_admins(&ctx.pool).await,
            1,
            "round {round}: {results:?}"
        );
        assert_eq!(
            results.iter().filter(|r| r.is_ok()).count(),
            1,
            "round {round}: {results:?}"
        );
        let refused = results.iter().find_map(|r| r.as_ref().err()).unwrap();
        assert!(
            matches!(refused, AppError::Validation { .. }),
            "{refused:?}"
        );
        users.set_suspended(a, false).await.unwrap();
        users.set_suspended(b, false).await.unwrap();
    }
}
