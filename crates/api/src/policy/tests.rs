//! Each policy against real rows and real principals: own and someone
//! else's entries in every visibility, media owned by another user, and an
//! API key whose owner holds `EditOthers` while the key does not.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![allow(missing_docs)]

use vyasa_common::AppError;
use vyasa_core::post::CreatePost;
use vyasa_core::user::Capability;
use vyasa_core::BlockDocument;
use vyasa_db::content_models::{MediaStorage, PostStatus, PostType};
use vyasa_db::models::Role;
use vyasa_db::repo::{ApiKeysRepo, MediaRepo, NewMedia, NewUser, UsersRepo};
use vyasa_testkit::TestDb;

use super::{
    account_as_self_or_managed, account_as_self_or_manager, account_for_management,
    full_administrator, may_grant, may_grant_role, media_for_edit, media_for_usage, media_for_view,
    media_owner_scope, media_reader, option_write, personal_data_within_reach, post_for_delete,
    post_for_edit, post_for_view, post_for_view_by_slug, post_list_scope, within_reach,
    PostListScope,
};
use crate::authz::tests::{test_state, TestState};
use crate::error::{ApiError, ApiResult};
use crate::middleware::api_key::ApiKeyPrincipal;
use crate::middleware::auth::CurrentUser;
use crate::middleware::Principal;
use crate::state::AppState;

pub(super) const ADMIN: i64 = 96_401;
pub(super) const EDITOR: i64 = 96_402;
pub(super) const AUTHOR: i64 = 96_403;
pub(super) const OTHER_AUTHOR: i64 = 96_404;
pub(super) const CONTRIBUTOR: i64 = 96_405;
pub(super) const SUBSCRIBER: i64 = 96_406;

const MISSING: i64 = 96_499;
const PASSWORD: &str = "open-sesame-96";

/// The rows every test decides about.
pub(super) struct Seeded {
    pub state: TestState,
    /// AUTHOR's draft.
    pub draft: i64,
    /// AUTHOR's published, open entry.
    pub published: i64,
    /// AUTHOR's published entry behind a password.
    pub protected: i64,
    /// AUTHOR's private entry behind a password.
    pub private: i64,
    /// AUTHOR's private entry with no password.
    pub private_open: i64,
    /// CONTRIBUTOR's draft.
    pub contributor_draft: i64,
    /// EDITOR's draft.
    pub editor_draft: i64,
    /// AUTHOR's upload.
    pub media: i64,
    /// A file owned by CONTRIBUTOR, who cannot upload (a demoted author).
    pub contributor_media: i64,
}

pub(super) async fn post(
    state: &AppState,
    author_id: i64,
    title: &str,
    status: PostStatus,
    password: Option<&str>,
) -> i64 {
    let row = state
        .posts
        .create(CreatePost {
            post_type: PostType::Post,
            status,
            title: title.to_owned(),
            slug: None,
            content: BlockDocument::new(Vec::new()),
            excerpt: None,
            author_id,
            parent_id: None,
            scheduled_for: None,
            password: password.map(str::to_owned),
            term_ids: None,
            layout: None,
        })
        .await
        .expect("seed post");
    assert_eq!(row.password_hash.is_some(), password.is_some());
    assert_eq!(row.status, status);
    row.id
}

async fn media(state: &AppState, id: i64, owner_id: i64) -> i64 {
    MediaRepo::new(state.pool.clone())
        .insert(&NewMedia {
            id,
            owner_id,
            file_name: "policy.png",
            mime: "image/png",
            byte_size: 1,
            storage: MediaStorage::Local,
            path: &format!("policy/{id}.png"),
            width: None,
            height: None,
            blurhash: None,
            alt: None,
            caption: None,
            derivatives: serde_json::json!({}),
            sha256: None,
        })
        .await
        .expect("seed media")
        .id
}

pub(super) async fn seeded(db: &TestDb) -> Seeded {
    let users = UsersRepo::new(db.pool().clone());
    for (id, name, role) in [
        (ADMIN, "admin", Role::Admin),
        (EDITOR, "editor", Role::Editor),
        (AUTHOR, "author", Role::Author),
        (OTHER_AUTHOR, "otherauthor", Role::Author),
        (CONTRIBUTOR, "contributor", Role::Contributor),
        (SUBSCRIBER, "subscriber", Role::Subscriber),
    ] {
        users
            .insert(&NewUser {
                id,
                email: &format!("{name}-policy@example.com"),
                username: &format!("{name}policy"),
                display_name: name,
                password_hash: None,
                role,
                bio: "",
            })
            .await
            .expect("seed user");
    }
    let state = test_state(db);
    let draft = post(&state, AUTHOR, "Draft", PostStatus::Draft, None).await;
    let published = post(&state, AUTHOR, "Published", PostStatus::Published, None).await;
    let protected = post(
        &state,
        AUTHOR,
        "Protected",
        PostStatus::Published,
        Some(PASSWORD),
    )
    .await;
    let private = post(
        &state,
        AUTHOR,
        "Private",
        PostStatus::Private,
        Some(PASSWORD),
    )
    .await;
    let private_open = post(&state, AUTHOR, "Private open", PostStatus::Private, None).await;
    let contributor_draft = post(&state, CONTRIBUTOR, "Pitch", PostStatus::Draft, None).await;
    let editor_draft = post(&state, EDITOR, "Editorial", PostStatus::Draft, None).await;
    let media_id = media(&state, 96_450, AUTHOR).await;
    let contributor_media = media(&state, 96_451, CONTRIBUTOR).await;
    Seeded {
        state,
        draft,
        published,
        protected,
        private,
        private_open,
        contributor_draft,
        editor_draft,
        media: media_id,
        contributor_media,
    }
}

/// `user_id` signed in with a browser session.
async fn as_user(state: &AppState, user_id: i64) -> Principal {
    Principal::Session(current(state, user_id).await)
}

async fn current(state: &AppState, user_id: i64) -> CurrentUser {
    CurrentUser {
        token: format!("policy-{user_id}"),
        user: state.users.get(user_id).await.expect("user"),
    }
}

/// The raw key of [`editor_key`], for requests.
pub(super) const EDITOR_KEY_RAW: &str = "vy_policy_editor_key_without_edit_others";

/// An API key owned by EDITOR (who holds `EditOthers`) that grants
/// `edit_posts` and `upload_media` only.
pub(super) async fn editor_key(state: &AppState) -> Principal {
    let hash = ApiKeyPrincipal::hash_raw(EDITOR_KEY_RAW);
    let keys = ApiKeysRepo::new(state.pool.clone());
    keys.insert(
        96_460,
        EDITOR,
        "no edit_others",
        &hash,
        &serde_json::json!(["edit_posts", "upload_media"]),
    )
    .await
    .expect("api key");
    let (key, user) = keys.resolve(&hash).await.expect("resolve key");
    Principal::ApiKey(ApiKeyPrincipal { key, user })
}

#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    Allowed,
    NotFound,
    Forbidden,
    BadToken,
}

fn outcome<T>(result: &ApiResult<T>) -> Outcome {
    match result {
        Ok(_) => Outcome::Allowed,
        Err(ApiError(AppError::NotFound { .. })) => Outcome::NotFound,
        Err(ApiError(AppError::Forbidden { .. })) => Outcome::Forbidden,
        Err(ApiError(AppError::Auth { .. })) => Outcome::BadToken,
        Err(ApiError(other)) => panic!("unexpected error: {other:?}"),
    }
}

#[tokio::test]
async fn post_for_edit_is_own_entry_or_edit_others() {
    let db = TestDb::new().await;
    let s = seeded(&db).await;
    let state = &s.state;
    let author = as_user(state, AUTHOR).await;
    let other = as_user(state, OTHER_AUTHOR).await;
    let editor = as_user(state, EDITOR).await;
    let contributor = as_user(state, CONTRIBUTOR).await;
    let subscriber = as_user(state, SUBSCRIBER).await;

    let own = post_for_edit(state, &author, s.draft).await;
    assert_eq!(outcome(&own), Outcome::Allowed);
    assert_eq!(own.ok().map(|p| p.id), Some(s.draft), "returns the row");
    assert_eq!(
        outcome(&post_for_edit(state, &other, s.draft).await),
        Outcome::Forbidden
    );
    assert_eq!(
        outcome(&post_for_edit(state, &editor, s.draft).await),
        Outcome::Allowed
    );
    assert_eq!(
        outcome(&post_for_edit(state, &contributor, s.contributor_draft).await),
        Outcome::Allowed
    );
    assert_eq!(
        outcome(&post_for_edit(state, &contributor, s.published).await),
        Outcome::Forbidden
    );
    assert_eq!(
        outcome(&post_for_edit(state, &subscriber, s.published).await),
        Outcome::Forbidden
    );
    // Existence is decided first: a missing entry is 404 for everyone.
    for principal in [&author, &editor, &subscriber] {
        assert_eq!(
            outcome(&post_for_edit(state, principal, MISSING).await),
            Outcome::NotFound
        );
    }
}

#[tokio::test]
async fn post_for_edit_holds_an_api_key_to_its_own_grants() {
    let db = TestDb::new().await;
    let s = seeded(&db).await;
    let state = &s.state;
    let key = editor_key(state).await;
    assert_eq!(
        outcome(&post_for_edit(state, &as_user(state, EDITOR).await, s.draft).await),
        Outcome::Allowed,
        "the owner may edit anyone's entry"
    );
    assert_eq!(
        outcome(&post_for_edit(state, &key, s.draft).await),
        Outcome::Forbidden,
        "the key was not granted edit_others"
    );
    assert_eq!(
        outcome(&post_for_edit(state, &key, s.editor_draft).await),
        Outcome::Allowed,
        "its owner's own entry needs edit_posts only"
    );
}

#[tokio::test]
async fn post_for_delete_adds_delete_posts_when_forced() {
    let db = TestDb::new().await;
    let s = seeded(&db).await;
    let state = &s.state;
    let author = as_user(state, AUTHOR).await;
    let other = as_user(state, OTHER_AUTHOR).await;
    let editor = as_user(state, EDITOR).await;
    let key = editor_key(state).await;

    assert_eq!(
        outcome(&post_for_delete(state, &author, s.draft, false).await),
        Outcome::Allowed,
        "trashing your own entry"
    );
    assert_eq!(
        outcome(&post_for_delete(state, &author, s.draft, true).await),
        Outcome::Forbidden,
        "an author has no delete_posts"
    );
    assert_eq!(
        outcome(&post_for_delete(state, &other, s.draft, false).await),
        Outcome::Forbidden
    );
    assert_eq!(
        outcome(&post_for_delete(state, &editor, s.draft, false).await),
        Outcome::Allowed
    );
    assert_eq!(
        outcome(&post_for_delete(state, &editor, s.draft, true).await),
        Outcome::Allowed
    );
    assert_eq!(
        outcome(&post_for_delete(state, &key, s.editor_draft, true).await),
        Outcome::Forbidden,
        "the key was not granted delete_posts"
    );
    assert_eq!(
        outcome(&post_for_delete(state, &key, s.draft, false).await),
        Outcome::Forbidden,
        "nor edit_others"
    );
    assert_eq!(
        outcome(&post_for_delete(state, &author, MISSING, true).await),
        Outcome::NotFound
    );
}

#[tokio::test]
async fn post_for_view_follows_status_ownership_and_tokens() {
    let db = TestDb::new().await;
    let s = seeded(&db).await;
    let state = &s.state;
    let author = as_user(state, AUTHOR).await;
    let other = as_user(state, OTHER_AUTHOR).await;
    let editor = as_user(state, EDITOR).await;
    let subscriber = as_user(state, SUBSCRIBER).await;
    let key = editor_key(state).await;

    // Published and open: anyone signed in.
    for principal in [&author, &other, &editor, &subscriber, &key] {
        assert_eq!(
            outcome(&post_for_view(state, principal, s.published, None).await),
            Outcome::Allowed
        );
    }
    // Everything else: the author, or `EditOthers`.
    for id in [s.draft, s.protected, s.private, s.private_open] {
        assert_eq!(
            outcome(&post_for_view(state, &author, id, None).await),
            Outcome::Allowed
        );
        assert_eq!(
            outcome(&post_for_view(state, &editor, id, None).await),
            Outcome::Allowed
        );
        for principal in [&other, &subscriber] {
            assert_eq!(
                outcome(&post_for_view(state, principal, id, None).await),
                Outcome::Forbidden
            );
        }
        assert_eq!(
            outcome(&post_for_view(state, &key, id, None).await),
            Outcome::Forbidden,
            "the key was not granted edit_others"
        );
    }
    assert_eq!(
        outcome(&post_for_view(state, &subscriber, MISSING, None).await),
        Outcome::NotFound
    );
}

#[tokio::test]
async fn post_for_view_accepts_a_read_token_only_for_protected_entries() {
    let db = TestDb::new().await;
    let s = seeded(&db).await;
    let state = &s.state;
    let subscriber = as_user(state, SUBSCRIBER).await;
    let token_for = |id: i64| async move {
        state
            .posts
            .verify_and_generate_token(id, PASSWORD, &state.private_token_secret)
            .await
            .expect("token")
    };

    for id in [s.protected, s.private] {
        let token = token_for(id).await;
        assert_eq!(
            outcome(&post_for_view(state, &subscriber, id, Some(&token)).await),
            Outcome::Allowed
        );
        assert_eq!(
            outcome(&post_for_view(state, &subscriber, id, Some("not-a-token")).await),
            Outcome::BadToken
        );
    }
    // A token for one entry does not open another.
    let token = token_for(s.protected).await;
    assert_eq!(
        outcome(&post_for_view(state, &subscriber, s.private, Some(&token)).await),
        Outcome::BadToken
    );
    // Entries with no password have no token path at all.
    for id in [s.draft, s.private_open] {
        assert_eq!(
            outcome(&post_for_view(state, &subscriber, id, Some(&token)).await),
            Outcome::Forbidden
        );
    }
}

#[tokio::test]
async fn post_for_view_by_slug_decides_like_by_id() {
    let db = TestDb::new().await;
    let s = seeded(&db).await;
    let state = &s.state;
    let author = as_user(state, AUTHOR).await;
    let subscriber = as_user(state, SUBSCRIBER).await;
    let draft_slug = state.posts.get(s.draft).await.expect("draft").slug;
    let published_slug = state.posts.get(s.published).await.expect("published").slug;

    let seen =
        post_for_view_by_slug(state, &subscriber, PostType::Post, &published_slug, None).await;
    assert_eq!(seen.ok().map(|p| p.id), Some(s.published));
    assert_eq!(
        outcome(
            &post_for_view_by_slug(state, &subscriber, PostType::Post, &draft_slug, None).await
        ),
        Outcome::Forbidden
    );
    assert_eq!(
        outcome(&post_for_view_by_slug(state, &author, PostType::Post, &draft_slug, None).await),
        Outcome::Allowed
    );
    assert_eq!(
        outcome(&post_for_view_by_slug(state, &author, PostType::Page, &draft_slug, None).await),
        Outcome::NotFound,
        "a slug belongs to its type"
    );
}

#[tokio::test]
async fn post_list_scope_widens_with_capabilities() {
    let db = TestDb::new().await;
    let s = seeded(&db).await;
    let state = &s.state;
    assert_eq!(
        post_list_scope(&as_user(state, EDITOR).await),
        PostListScope {
            all: true,
            own_author: Some(EDITOR)
        }
    );
    assert_eq!(
        post_list_scope(&as_user(state, AUTHOR).await),
        PostListScope {
            all: false,
            own_author: Some(AUTHOR)
        }
    );
    assert_eq!(
        post_list_scope(&as_user(state, SUBSCRIBER).await),
        PostListScope {
            all: false,
            own_author: None
        }
    );
    assert_eq!(
        post_list_scope(&editor_key(state).await),
        PostListScope {
            all: false,
            own_author: Some(EDITOR)
        },
        "the key was not granted edit_others"
    );
}

#[tokio::test]
async fn media_for_edit_is_own_upload_or_edit_others() {
    let db = TestDb::new().await;
    let s = seeded(&db).await;
    let state = &s.state;
    let author = as_user(state, AUTHOR).await;
    let other = as_user(state, OTHER_AUTHOR).await;
    let editor = as_user(state, EDITOR).await;
    let contributor = as_user(state, CONTRIBUTOR).await;
    let key = editor_key(state).await;

    let own = media_for_edit(state, &author, s.media).await;
    assert_eq!(own.ok().map(|m| m.id), Some(s.media), "returns the row");
    assert_eq!(
        outcome(&media_for_edit(state, &other, s.media).await),
        Outcome::Forbidden
    );
    assert_eq!(
        outcome(&media_for_edit(state, &editor, s.media).await),
        Outcome::Allowed
    );
    assert_eq!(
        outcome(&media_for_edit(state, &key, s.media).await),
        Outcome::Forbidden,
        "the key was not granted edit_others"
    );
    assert_eq!(
        outcome(&media_for_edit(state, &contributor, s.contributor_media).await),
        Outcome::Forbidden,
        "owning a file is not enough without upload_media"
    );
    // Existence is decided first, even for a caller who cannot upload.
    for principal in [&author, &contributor] {
        assert_eq!(
            outcome(&media_for_edit(state, principal, MISSING).await),
            Outcome::NotFound
        );
    }
}

#[tokio::test]
async fn media_for_view_is_any_library_reader() {
    let db = TestDb::new().await;
    let s = seeded(&db).await;
    let state = &s.state;
    let other = as_user(state, OTHER_AUTHOR).await;
    let contributor = as_user(state, CONTRIBUTOR).await;
    let subscriber = as_user(state, SUBSCRIBER).await;

    for principal in [&other, &contributor, &editor_key(state).await] {
        assert_eq!(
            outcome(&media_for_view(state, principal, s.media).await),
            Outcome::Allowed,
            "someone else's upload is readable"
        );
    }
    assert_eq!(
        outcome(&media_for_view(state, &subscriber, s.media).await),
        Outcome::Forbidden
    );
    // The reader check comes first: a non-reader learns nothing about ids.
    assert_eq!(
        outcome(&media_for_view(state, &subscriber, MISSING).await),
        Outcome::Forbidden
    );
    assert_eq!(
        outcome(&media_for_view(state, &contributor, MISSING).await),
        Outcome::NotFound
    );
    assert!(media_reader(&contributor).is_ok());
    assert!(media_reader(&subscriber).is_err());
}

#[tokio::test]
async fn media_for_usage_is_own_upload_or_edit_others_for_readers() {
    let db = TestDb::new().await;
    let s = seeded(&db).await;
    let state = &s.state;
    let author = as_user(state, AUTHOR).await;
    let other = as_user(state, OTHER_AUTHOR).await;
    let editor = as_user(state, EDITOR).await;
    let contributor = as_user(state, CONTRIBUTOR).await;
    let subscriber = as_user(state, SUBSCRIBER).await;

    assert_eq!(
        outcome(&media_for_usage(state, &author, s.media).await),
        Outcome::Allowed
    );
    assert_eq!(
        outcome(&media_for_usage(state, &other, s.media).await),
        Outcome::Forbidden
    );
    assert_eq!(
        outcome(&media_for_usage(state, &contributor, s.media).await),
        Outcome::Forbidden
    );
    assert_eq!(
        outcome(&media_for_usage(state, &editor, s.media).await),
        Outcome::Allowed
    );
    assert_eq!(
        outcome(&media_for_usage(state, &editor_key(state).await, s.media).await),
        Outcome::Forbidden,
        "the key was not granted edit_others"
    );
    // Where it parts from `media_for_edit`: no `upload_media` needed for
    // a file you own, and the reader check precedes existence.
    assert_eq!(
        outcome(&media_for_usage(state, &contributor, s.contributor_media).await),
        Outcome::Allowed
    );
    assert_eq!(
        outcome(&media_for_usage(state, &subscriber, MISSING).await),
        Outcome::Forbidden
    );
    assert_eq!(
        outcome(&media_for_usage(state, &contributor, MISSING).await),
        Outcome::NotFound
    );
}

#[tokio::test]
async fn media_owner_scope_is_own_uploads_without_edit_others() {
    let db = TestDb::new().await;
    let s = seeded(&db).await;
    let state = &s.state;
    let editor = as_user(state, EDITOR).await;
    let author = as_user(state, AUTHOR).await;
    assert_eq!(media_owner_scope(&editor, None), None);
    assert_eq!(media_owner_scope(&editor, Some(AUTHOR)), Some(AUTHOR));
    assert_eq!(media_owner_scope(&author, None), Some(AUTHOR));
    assert_eq!(media_owner_scope(&author, Some(EDITOR)), Some(AUTHOR));
    assert_eq!(
        media_owner_scope(&editor_key(state).await, Some(AUTHOR)),
        Some(EDITOR),
        "the key was not granted edit_others"
    );
}

#[tokio::test]
async fn an_account_is_its_holders_or_a_user_managers() {
    let db = TestDb::new().await;
    let s = seeded(&db).await;
    let state = &s.state;
    let editor = current(state, EDITOR).await;
    let admin = current(state, ADMIN).await;
    assert!(account_as_self_or_manager(&editor, EDITOR).is_ok());
    assert!(matches!(
        account_as_self_or_manager(&editor, ADMIN),
        Err(AppError::Forbidden { .. })
    ));
    assert!(account_as_self_or_manager(&admin, EDITOR).is_ok());
    assert!(
        account_as_self_or_manager(&admin, MISSING).is_ok(),
        "existence is the caller's next question, not this one's"
    );
}

fn refused<T>(result: ApiResult<T>) -> Option<AppError> {
    result.err().map(|ApiError(err)| err)
}

#[tokio::test]
async fn managing_an_account_takes_manage_users_and_an_account_within_reach() {
    let db = TestDb::new().await;
    let s = seeded(&db).await;
    let state = &s.state;
    let admin = current(state, ADMIN).await;
    let editor = current(state, EDITOR).await;
    // An editor given manage_users by a per-user override: a user manager
    // who is not an administrator.
    sqlx::query("UPDATE users SET meta = '{\"enabled_caps\": [\"manage_users\"]}' WHERE id = $1")
        .bind(EDITOR)
        .execute(&state.pool)
        .await
        .unwrap();
    let manager = current(state, EDITOR).await;

    // Without the capability: refused before the account is looked up.
    for id in [ADMIN, SUBSCRIBER, EDITOR, MISSING] {
        assert!(matches!(
            refused(account_for_management(state, &editor, id).await),
            Some(AppError::Forbidden { .. })
        ));
    }
    // With it: accounts that hold no more than the caller, themselves
    // included.
    for id in [EDITOR, AUTHOR, CONTRIBUTOR, SUBSCRIBER] {
        let row = account_for_management(state, &manager, id).await;
        assert_eq!(row.ok().map(|row| row.id), Some(id));
    }
    assert!(matches!(
        refused(account_for_management(state, &manager, ADMIN).await),
        Some(AppError::Forbidden { .. })
    ));
    assert!(matches!(
        refused(account_for_management(state, &manager, MISSING).await),
        Some(AppError::NotFound { .. })
    ));
    // An administrator reaches everyone.
    for id in [ADMIN, EDITOR, SUBSCRIBER] {
        assert!(account_for_management(state, &admin, id).await.is_ok());
    }

    // One's own account needs no capability; anyone else's is managed.
    let subscriber = current(state, SUBSCRIBER).await;
    assert!(account_as_self_or_managed(state, &subscriber, SUBSCRIBER)
        .await
        .is_ok());
    assert!(matches!(
        refused(account_as_self_or_managed(state, &subscriber, MISSING).await),
        Some(AppError::Forbidden { .. })
    ));
    assert!(matches!(
        refused(account_as_self_or_managed(state, &manager, ADMIN).await),
        Some(AppError::Forbidden { .. })
    ));
    assert!(account_as_self_or_managed(state, &manager, AUTHOR)
        .await
        .is_ok());
}

#[tokio::test]
async fn reach_and_grants_are_measured_against_a_keys_own_grants() {
    let db = TestDb::new().await;
    let s = seeded(&db).await;
    let state = &s.state;
    let editor = as_user(state, EDITOR).await;
    let key = editor_key(state).await;
    let author = state.users.get(AUTHOR).await.unwrap();
    let subscriber = state.users.get(SUBSCRIBER).await.unwrap();

    // The editor holds all an author does; their key was not granted
    // `publish_posts`, so the key does not.
    assert!(within_reach(&editor, &author).is_ok());
    let err = within_reach(&key, &author).unwrap_err();
    assert!(err.to_string().contains("publish_posts"), "{err}");
    assert!(may_grant(&editor, &[Capability::EditOthers]).is_ok());
    assert!(may_grant(&key, &[Capability::EditPosts, Capability::UploadMedia]).is_ok());
    assert!(matches!(
        may_grant(&key, &[Capability::EditPosts, Capability::EditOthers]),
        Err(AppError::Forbidden { .. })
    ));
    // A role that holds nothing is anyone's to give.
    assert!(may_grant(&key, &[]).is_ok());
    assert!(matches!(
        within_reach(&as_user(state, SUBSCRIBER).await, &author),
        Err(AppError::Forbidden { .. })
    ));
    assert!(within_reach(&as_user(state, AUTHOR).await, &subscriber).is_ok());

    // By name: built-in roles are measured like any other.
    assert!(may_grant_role(state, &editor, "author").await.is_ok());
    assert!(matches!(
        refused(may_grant_role(state, &editor, "admin").await),
        Some(AppError::Forbidden { .. })
    ));
    assert!(matches!(
        refused(may_grant_role(state, &editor, "no-such-role").await),
        Some(AppError::Validation { .. })
    ));

    // Erasure: an address with no account is nobody's to guard.
    assert!(
        personal_data_within_reach(state, &editor, "nobody@example.com")
            .await
            .is_ok()
    );
    assert!(
        personal_data_within_reach(state, &editor, " Author-Policy@example.com ")
            .await
            .is_ok()
    );
    assert!(matches!(
        refused(personal_data_within_reach(state, &editor, "admin-policy@example.com").await),
        Some(AppError::Forbidden { .. })
    ));
    assert!(matches!(
        refused(personal_data_within_reach(state, &key, "author-policy@example.com").await),
        Some(AppError::Forbidden { .. })
    ));
}

#[tokio::test]
async fn a_full_administrator_holds_every_capability() {
    let db = TestDb::new().await;
    let s = seeded(&db).await;
    let state = &s.state;
    let admin = as_user(state, ADMIN).await;

    assert!(full_administrator(&admin, "change the mail relay").is_ok());
    for id in [EDITOR, AUTHOR, CONTRIBUTOR, SUBSCRIBER] {
        let err = full_administrator(&as_user(state, id).await, "change the mail relay")
            .expect_err("not a full administrator");
        assert!(matches!(err, AppError::Forbidden { .. }), "{err:?}");
        assert!(
            err.to_string()
                .contains("only a full administrator can change the mail relay"),
            "{err}"
        );
    }

    // An administrator's key is held to its grants: every one, or not a
    // full administrator.
    let keys = ApiKeysRepo::new(state.pool.clone());
    let mut key_with = Vec::new();
    for (id, raw, grants) in [
        (
            96_470,
            "vy_policy_admin_options_key",
            serde_json::json!(["manage_options", "view_admin"]),
        ),
        (
            96_471,
            "vy_policy_admin_full_key",
            serde_json::json!(Capability::ALL.map(vyasa_core::user::cap_name)),
        ),
    ] {
        let hash = ApiKeyPrincipal::hash_raw(raw);
        keys.insert(id, ADMIN, raw, &hash, &grants)
            .await
            .expect("api key");
        let (key, user) = keys.resolve(&hash).await.expect("resolve key");
        key_with.push(Principal::ApiKey(ApiKeyPrincipal { key, user }));
    }
    assert!(matches!(
        full_administrator(&key_with[0], "import a site archive"),
        Err(AppError::Forbidden { .. })
    ));
    assert!(full_administrator(&key_with[1], "import a site archive").is_ok());

    // Options: the ones that redirect mail, links, updates or package
    // trust take a full administrator; the rest take what the route does.
    let narrow = &key_with[0];
    assert!(option_write(narrow, ["site_title", "posts_per_page"]).is_ok());
    assert!(option_write(narrow, [] as [&str; 0]).is_ok());
    for key in vyasa_core::options::FULL_ADMINISTRATOR_OPTION_KEYS {
        let err = option_write(narrow, [*key]).expect_err(key);
        assert!(matches!(err, AppError::Forbidden { .. }), "{key}: {err:?}");
        assert!(err.to_string().contains(key), "{err}");
        assert!(option_write(&admin, [*key]).is_ok(), "{key}");
    }
    let err = option_write(narrow, ["site_title", "site_url", "smtp_host"]).expect_err("mixed");
    let message = err.to_string();
    assert!(
        message.contains("only a full administrator can change"),
        "{message}"
    );
    assert!(
        message.contains("\"site_url\"") && message.contains("\"smtp_host\""),
        "{message}"
    );
    assert!(!message.contains("site_title"), "{message}");
}
