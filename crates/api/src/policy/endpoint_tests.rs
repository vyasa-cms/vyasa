//! The entry sub-resources that used to decide for themselves (language,
//! the editing lock, the term list) follow the entry's own policies.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![allow(missing_docs)]

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use tower::ServiceExt;
use vyasa_db::content_models::{PostStatus, PostType};
use vyasa_testkit::TestDb;

use super::tests::{
    editor_key, post as seed_post, seeded, AUTHOR, EDITOR, EDITOR_KEY_RAW, OTHER_AUTHOR, SUBSCRIBER,
};
use crate::authz::tests::session;
use crate::middleware::auth::SESSION_COOKIE;
use crate::rest;
use crate::state::AppState;

const MISSING: i64 = 96_498;

enum As<'a> {
    Cookie(&'a str),
    Key(&'a str),
}

async fn call(
    app: &Router,
    caller: &As<'_>,
    method: Method,
    path: &str,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    let mut request = Request::builder().method(method).uri(path);
    request = match caller {
        As::Cookie(token) => request.header(header::COOKIE, format!("{SESSION_COOKIE}={token}")),
        As::Key(raw) => request.header(header::AUTHORIZATION, format!("Bearer {raw}")),
    };
    let body = match body {
        Some(json) => {
            request = request.header(header::CONTENT_TYPE, "application/json");
            Body::from(json.to_string())
        }
        None => Body::empty(),
    };
    let response = app
        .clone()
        .oneshot(request.body(body).expect("request"))
        .await
        .expect("infallible");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 256 * 1024)
        .await
        .expect("body");
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
    )
}

async fn status(app: &Router, caller: &As<'_>, method: Method, path: &str) -> StatusCode {
    call(app, caller, method, path, None).await.0
}

struct Callers {
    author: String,
    other: String,
    editor: String,
    subscriber: String,
}

async fn callers(state: &AppState) -> Callers {
    editor_key(state).await;
    Callers {
        author: session(state, AUTHOR).await,
        other: session(state, OTHER_AUTHOR).await,
        editor: session(state, EDITOR).await,
        subscriber: session(state, SUBSCRIBER).await,
    }
}

#[tokio::test]
async fn reading_an_entrys_language_takes_seeing_the_entry() {
    let db = TestDb::new().await;
    let s = seeded(&db).await;
    let app = rest::router().with_state((*s.state).clone());
    let c = callers(&s.state).await;
    let get = |caller: As<'static>, id: i64| {
        let app = app.clone();
        async move { status(&app, &caller, Method::GET, &format!("/posts/{id}/language")).await }
    };
    let cookie = |token: &str| As::Cookie(Box::leak(token.to_owned().into_boxed_str()));

    for id in [s.draft, s.protected, s.private, s.private_open] {
        assert_eq!(get(cookie(&c.subscriber), id).await, StatusCode::FORBIDDEN);
        assert_eq!(get(cookie(&c.other), id).await, StatusCode::FORBIDDEN);
        assert_eq!(
            get(As::Key(EDITOR_KEY_RAW), id).await,
            StatusCode::FORBIDDEN
        );
        assert_eq!(get(cookie(&c.author), id).await, StatusCode::OK);
        assert_eq!(get(cookie(&c.editor), id).await, StatusCode::OK);
    }
    assert_eq!(
        get(cookie(&c.subscriber), s.published).await,
        StatusCode::OK
    );
    assert_eq!(
        get(cookie(&c.subscriber), MISSING).await,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn setting_an_entrys_language_takes_editing_the_entry() {
    let db = TestDb::new().await;
    let s = seeded(&db).await;
    let app = rest::router().with_state((*s.state).clone());
    let c = callers(&s.state).await;
    let path = format!("/posts/{}/language", s.draft);
    let put = |caller: As<'static>, lang: &'static str| {
        let (app, path) = (app.clone(), path.clone());
        async move {
            call(
                &app,
                &caller,
                Method::PUT,
                &path,
                Some(serde_json::json!({ "lang": lang })),
            )
            .await
            .0
        }
    };
    let cookie = |token: &str| As::Cookie(Box::leak(token.to_owned().into_boxed_str()));

    assert_eq!(put(cookie(&c.author), "fr").await, StatusCode::OK);
    assert_eq!(put(cookie(&c.other), "de").await, StatusCode::FORBIDDEN);
    assert_eq!(
        put(As::Key(EDITOR_KEY_RAW), "de").await,
        StatusCode::FORBIDDEN,
        "the key was not granted edit_others"
    );
    let (_, body) = call(&app, &As::Cookie(&c.author), Method::GET, &path, None).await;
    assert_eq!(body["lang"], "fr", "a refused write changes nothing");
    assert_eq!(put(cookie(&c.editor), "es").await, StatusCode::OK);
    let (_, body) = call(&app, &As::Cookie(&c.author), Method::GET, &path, None).await;
    assert_eq!(body["lang"], "es");
}

#[tokio::test]
async fn the_editing_lock_is_for_those_who_may_edit_the_entry() {
    let db = TestDb::new().await;
    let s = seeded(&db).await;
    let app = rest::router().with_state((*s.state).clone());
    let c = callers(&s.state).await;
    let path = format!("/posts/{}/lock", s.draft);
    let forced = format!("{path}?force=true");

    let (code, body) = call(&app, &As::Cookie(&c.author), Method::POST, &path, None).await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(body["mine"], true);

    // Another author can neither ask nor take over. Letting go only ever
    // touches the caller's own lock, so it is a quiet no-op for them.
    let other = As::Cookie(&c.other);
    assert_eq!(
        status(&app, &other, Method::POST, &path).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        status(&app, &other, Method::POST, &forced).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        status(&app, &other, Method::DELETE, &path).await,
        StatusCode::NO_CONTENT
    );
    let key = As::Key(EDITOR_KEY_RAW);
    assert_eq!(
        status(&app, &key, Method::POST, &forced).await,
        StatusCode::FORBIDDEN,
        "the key was not granted edit_others"
    );
    assert_eq!(
        status(&app, &key, Method::DELETE, &path).await,
        StatusCode::NO_CONTENT
    );
    let (_, body) = call(&app, &As::Cookie(&c.author), Method::POST, &path, None).await;
    assert_eq!(body["mine"], true, "the author still holds it");

    // An editor may edit it, so sees the holder and can take over.
    let editor = As::Cookie(&c.editor);
    let (code, body) = call(&app, &editor, Method::POST, &path, None).await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(body["mine"], false);
    let (code, body) = call(&app, &editor, Method::POST, &forced, None).await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(body["mine"], true);
    assert_eq!(
        status(&app, &editor, Method::DELETE, &path).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        status(&app, &As::Cookie(&c.author), Method::DELETE, &path).await,
        StatusCode::NO_CONTENT,
        "releasing a lock you do not hold is still a quiet no-op"
    );
    assert_eq!(
        status(
            &app,
            &editor,
            Method::DELETE,
            &format!("/posts/{MISSING}/lock")
        )
        .await,
        StatusCode::NO_CONTENT,
        "there is no lock to let go of on an entry that does not exist"
    );
}

/// Someone who held the lock and may no longer edit the entry (a changed
/// role, an entry handed to someone else) must still be able to let go.
#[tokio::test]
async fn a_holder_who_may_not_edit_can_still_release_their_lock() {
    let db = TestDb::new().await;
    let s = seeded(&db).await;
    let app = rest::router().with_state((*s.state).clone());
    let c = callers(&s.state).await;
    let path = format!("/posts/{}/lock", s.draft);
    crate::locks::acquire(&s.state.pool, s.draft, OTHER_AUTHOR, "otherauthor", true)
        .await
        .expect("lock");
    let (_, body) = call(&app, &As::Cookie(&c.author), Method::POST, &path, None).await;
    assert_eq!(body["mine"], false, "held by the other author");

    assert_eq!(
        status(&app, &As::Cookie(&c.other), Method::DELETE, &path).await,
        StatusCode::NO_CONTENT
    );
    let (_, body) = call(&app, &As::Cookie(&c.author), Method::POST, &path, None).await;
    assert_eq!(body["mine"], true, "the lock was released");
}

#[tokio::test]
async fn reading_an_entrys_terms_takes_seeing_the_entry() {
    let db = TestDb::new().await;
    let s = seeded(&db).await;
    let app = rest::router().with_state((*s.state).clone());
    let c = callers(&s.state).await;
    let get = |caller: As<'static>, id: i64| {
        let app = app.clone();
        async move { status(&app, &caller, Method::GET, &format!("/posts/{id}/terms")).await }
    };
    let cookie = |token: &str| As::Cookie(Box::leak(token.to_owned().into_boxed_str()));

    // An API key is held to its grants here too.
    assert_eq!(
        get(As::Key(EDITOR_KEY_RAW), s.draft).await,
        StatusCode::FORBIDDEN,
        "the key was not granted edit_others"
    );
    // A password-protected entry is not open just because it is published.
    assert_eq!(
        get(cookie(&c.subscriber), s.protected).await,
        StatusCode::FORBIDDEN
    );
    for id in [s.draft, s.private, s.private_open] {
        assert_eq!(get(cookie(&c.subscriber), id).await, StatusCode::FORBIDDEN);
        assert_eq!(get(cookie(&c.other), id).await, StatusCode::FORBIDDEN);
    }
    for id in [s.draft, s.protected, s.private, s.private_open, s.published] {
        assert_eq!(get(cookie(&c.author), id).await, StatusCode::OK);
        assert_eq!(get(cookie(&c.editor), id).await, StatusCode::OK);
    }
    assert_eq!(
        get(cookie(&c.subscriber), s.published).await,
        StatusCode::OK
    );
    assert_eq!(
        get(cookie(&c.subscriber), MISSING).await,
        StatusCode::NOT_FOUND
    );
}

/// The translation group `id` belongs to, if any.
async fn group_of(state: &AppState, id: i64) -> Option<i64> {
    state.posts.get(id).await.expect("post").translation_group
}

/// A translation link is for entries the caller can see: linking says the
/// target exists and changes its group, so it takes the viewing rule (no
/// read token), not the editing rule — an author translates an editor's
/// published entry.
#[tokio::test]
async fn translation_of_takes_seeing_the_entry_linked_to() {
    let db = TestDb::new().await;
    let s = seeded(&db).await;
    let app = rest::router().with_state((*s.state).clone());
    let c = callers(&s.state).await;
    let theirs = seed_post(&s.state, OTHER_AUTHOR, "Theirs", PostStatus::Draft, None).await;
    let link = |caller: As<'static>, id: i64, target: i64| {
        let app = app.clone();
        async move {
            call(
                &app,
                &caller,
                Method::PUT,
                &format!("/posts/{id}"),
                Some(serde_json::json!({ "translation_of": target })),
            )
            .await
            .0
        }
    };
    let cookie = |token: &str| As::Cookie(Box::leak(token.to_owned().into_boxed_str()));

    // Entries the caller cannot see: refused, and neither entry is touched.
    for hidden in [s.draft, s.private, s.private_open, s.protected] {
        assert_eq!(
            link(cookie(&c.other), theirs, hidden).await,
            StatusCode::FORBIDDEN
        );
        assert_eq!(group_of(&s.state, hidden).await, None);
    }
    assert_eq!(group_of(&s.state, theirs).await, None);
    assert_eq!(
        link(cookie(&c.author), s.draft, s.contributor_draft).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(group_of(&s.state, s.contributor_draft).await, None);
    assert_eq!(group_of(&s.state, s.draft).await, None);
    assert_eq!(
        link(As::Key(EDITOR_KEY_RAW), s.editor_draft, s.contributor_draft).await,
        StatusCode::FORBIDDEN,
        "the key was not granted edit_others, so it does not see the draft"
    );
    assert_eq!(group_of(&s.state, s.contributor_draft).await, None);
    // A missing target answers as the entry itself would.
    assert_eq!(
        link(cookie(&c.author), s.draft, MISSING).await,
        StatusCode::NOT_FOUND
    );
    // Creating an entry as a translation is the same decision, made
    // before anything is written.
    let create = |caller: As<'static>, title: &'static str, target: i64| {
        let app = app.clone();
        async move {
            call(
                &app,
                &caller,
                Method::POST,
                "/posts",
                Some(serde_json::json!({
                    "title": title,
                    "content": { "schema_version": 1, "blocks": [] },
                    "translation_of": target,
                })),
            )
            .await
            .0
        }
    };
    assert_eq!(
        create(cookie(&c.other), "Traduction", s.draft).await,
        StatusCode::FORBIDDEN
    );
    assert!(
        s.state
            .posts
            .get_by_slug(PostType::Post, "traduction")
            .await
            .is_err(),
        "a refused create leaves no entry behind"
    );
    assert_eq!(group_of(&s.state, s.draft).await, None);

    // Another author's published entry is visible, so it can be translated.
    assert_eq!(
        link(cookie(&c.other), theirs, s.published).await,
        StatusCode::OK
    );
    assert_eq!(group_of(&s.state, theirs).await, Some(s.published));
    assert_eq!(
        create(cookie(&c.other), "Traduction", s.published).await,
        StatusCode::CREATED
    );
    // Your own entries, and anyone's for an editor.
    assert_eq!(
        link(cookie(&c.author), s.draft, s.published).await,
        StatusCode::OK
    );
    assert_eq!(group_of(&s.state, s.draft).await, Some(s.published));
    assert_eq!(
        link(cookie(&c.editor), s.editor_draft, s.contributor_draft).await,
        StatusCode::OK
    );
    assert_eq!(
        group_of(&s.state, s.contributor_draft).await,
        Some(s.contributor_draft)
    );
}

#[tokio::test]
async fn link_slug_takes_seeing_the_entry_linked_to() {
    let db = TestDb::new().await;
    let s = seeded(&db).await;
    let state = &s.state;
    let app = rest::router().with_state((**state).clone());
    let c = callers(state).await;
    let theirs = seed_post(state, OTHER_AUTHOR, "Theirs", PostStatus::Draft, None).await;
    // Every target but `private` has declared a language, so only
    // permission is at stake.
    let mut slug = std::collections::HashMap::new();
    for id in [
        s.published,
        s.private_open,
        s.contributor_draft,
        s.editor_draft,
    ] {
        state.translations.set_lang(id, "en").await.expect("lang");
        slug.insert(id, state.posts.get(id).await.expect("post").slug);
    }
    slug.insert(
        s.private,
        state.posts.get(s.private).await.expect("post").slug,
    );
    let link_to = |caller: As<'static>, id: i64, slug: String| {
        let app = app.clone();
        async move {
            call(
                &app,
                &caller,
                Method::PUT,
                &format!("/posts/{id}/language"),
                Some(serde_json::json!({ "lang": "fr", "link_slug": slug })),
            )
            .await
        }
    };
    let link =
        |caller: As<'static>, id: i64, target: i64| link_to(caller, id, slug[&target].clone());
    let cookie = |token: &str| As::Cookie(Box::leak(token.to_owned().into_boxed_str()));
    let untouched =
        |id: i64| async move { state.translations.get(id).await.expect("get").is_none() };

    // An unknown slug is the validation error it always was.
    let (code, unknown) = link_to(cookie(&c.other), theirs, "no-such-entry".to_owned()).await;
    assert_eq!(code, StatusCode::BAD_REQUEST);
    assert!(untouched(theirs).await);
    // A slug the caller cannot see gets the very same answer: nothing says
    // that it exists, and nothing is written.
    for (hidden, title) in [(s.contributor_draft, "Pitch"), (s.private_open, "Private")] {
        let (code, body) = link(cookie(&c.other), theirs, hidden).await;
        assert_eq!(code, StatusCode::BAD_REQUEST, "{body}");
        assert!(
            !body.to_string().contains(title),
            "the refusal does not name the entry: {body}"
        );
        let expected = unknown["message"]
            .as_str()
            .expect("message")
            .replace("no-such-entry", &slug[&hidden]);
        assert_eq!(body["code"], unknown["code"]);
        assert_eq!(body["message"], expected);
        assert!(untouched(theirs).await, "a refused link changes nothing");
    }
    let (code, _) = link(As::Key(EDITOR_KEY_RAW), s.editor_draft, s.contributor_draft).await;
    assert_eq!(
        code,
        StatusCode::BAD_REQUEST,
        "the key was not granted edit_others, so it does not see the draft"
    );
    let after = state.translations.get(s.editor_draft).await.expect("get");
    assert_eq!(
        after.map(|t| t.lang).as_deref(),
        Some("en"),
        "a refused link changes nothing"
    );
    // A visible target that has no language yet: refused before anything
    // is written.
    let (code, body) = link(cookie(&c.author), s.draft, s.private).await;
    assert_eq!(code, StatusCode::BAD_REQUEST, "{body}");
    assert!(untouched(s.draft).await, "an invalid link changes nothing");

    // Another author's published entry, your own, and anyone's for an editor.
    let (code, body) = link(cookie(&c.other), theirs, s.published).await;
    assert_eq!(code, StatusCode::OK, "{body}");
    assert_eq!(body["lang"], "fr");
    let (code, body) = link(cookie(&c.author), s.draft, s.published).await;
    assert_eq!(code, StatusCode::OK, "{body}");
    let (code, body) = link(cookie(&c.editor), s.editor_draft, s.contributor_draft).await;
    assert_eq!(code, StatusCode::OK, "{body}");
}

/// Both AI actions on a file decide alike.
async fn ai_action_takes_editing_the_file(action: &str) {
    let db = TestDb::new().await;
    let s = seeded(&db).await;
    let app = rest::router().with_state((*s.state).clone());
    let c = callers(&s.state).await;
    let path = format!("/media/{}/{action}", s.media);
    assert_eq!(
        status(&app, &As::Cookie(&c.other), Method::POST, &path).await,
        StatusCode::FORBIDDEN,
        "someone else's upload"
    );
    assert_eq!(
        status(&app, &As::Key(EDITOR_KEY_RAW), Method::POST, &path).await,
        StatusCode::FORBIDDEN,
        "the key was not granted edit_others"
    );
    // Owner and editor get past the policy; the feature is off in a fresh
    // database, which is the 400 they meet next.
    for token in [&c.author, &c.editor] {
        assert_eq!(
            status(&app, &As::Cookie(token), Method::POST, &path).await,
            StatusCode::BAD_REQUEST
        );
    }
    let missing = format!("/media/{MISSING}/{action}");
    assert_eq!(
        status(&app, &As::Cookie(&c.author), Method::POST, &missing).await,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn writing_alt_text_takes_editing_the_file() {
    ai_action_takes_editing_the_file("alt-text").await;
}

#[tokio::test]
async fn transcribing_takes_editing_the_file() {
    ai_action_takes_editing_the_file("transcribe").await;
}

#[tokio::test]
async fn a_transcript_is_for_library_readers() {
    let db = TestDb::new().await;
    let s = seeded(&db).await;
    let app = rest::router().with_state((*s.state).clone());
    let c = callers(&s.state).await;
    vyasa_db::repo::AiDataRepo::new(s.state.pool.clone())
        .set_transcript(s.media, "spoken words")
        .await
        .expect("transcript");
    let path = format!("/media/{}/transcript", s.media);

    assert_eq!(
        status(&app, &As::Cookie(&c.subscriber), Method::GET, &path).await,
        StatusCode::FORBIDDEN
    );
    // Like the file's metadata, readable by any library reader.
    for token in [&c.author, &c.other, &c.editor] {
        let (code, body) = call(&app, &As::Cookie(token), Method::GET, &path, None).await;
        assert_eq!(code, StatusCode::OK);
        assert_eq!(body["transcript"], "spoken words");
    }
    assert_eq!(
        status(
            &app,
            &As::Cookie(&c.author),
            Method::GET,
            &format!("/media/{MISSING}/transcript")
        )
        .await,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn an_entrys_recording_takes_seeing_the_entry() {
    let db = TestDb::new().await;
    let s = seeded(&db).await;
    let app = rest::router().with_state((*s.state).clone());
    let c = callers(&s.state).await;
    let get = |caller: As<'static>, id: i64| {
        let app = app.clone();
        async move { status(&app, &caller, Method::GET, &format!("/posts/{id}/audio")).await }
    };
    let cookie = |token: &str| As::Cookie(Box::leak(token.to_owned().into_boxed_str()));

    for id in [s.draft, s.protected, s.private, s.private_open] {
        assert_eq!(get(cookie(&c.subscriber), id).await, StatusCode::FORBIDDEN);
        assert_eq!(get(cookie(&c.other), id).await, StatusCode::FORBIDDEN);
        assert_eq!(get(cookie(&c.author), id).await, StatusCode::OK);
        assert_eq!(get(cookie(&c.editor), id).await, StatusCode::OK);
    }
    assert_eq!(
        get(cookie(&c.subscriber), s.published).await,
        StatusCode::OK
    );
    assert_eq!(
        get(cookie(&c.subscriber), MISSING).await,
        StatusCode::NOT_FOUND
    );
}

/// The ids an entry's response lists as its translations.
async fn translations_of(app: &Router, caller: &As<'_>, id: i64) -> Vec<i64> {
    let (status, body) = call(app, caller, Method::GET, &format!("/posts/{id}"), None).await;
    assert_eq!(status, StatusCode::OK);
    let mut ids: Vec<i64> = body["translations"]
        .as_array()
        .expect("translations")
        .iter()
        .map(|m| m["id"].as_i64().expect("member id"))
        .collect();
    ids.sort_unstable();
    ids
}

/// The translations an entry lists are entries themselves: a caller is
/// shown the members they could open, not every member of the group.
#[tokio::test]
async fn a_translation_group_lists_only_the_members_the_caller_sees() {
    let db = TestDb::new().await;
    let s = seeded(&db).await;
    let state = &s.state;
    let app = rest::router().with_state((**state).clone());
    let c = callers(state).await;
    let open = seed_post(state, OTHER_AUTHOR, "Abierto", PostStatus::Published, None).await;
    let theirs = seed_post(state, OTHER_AUTHOR, "Brouillon", PostStatus::Draft, None).await;
    let repo = vyasa_db::repo::PostsRepo::new(state.pool.clone());
    for (id, lang) in [
        (s.published, "en"),
        (open, "es"),
        (theirs, "fr"),
        (s.protected, "de"),
    ] {
        repo.set_language(id, lang, Some(s.published))
            .await
            .expect("group");
    }
    let mut everything = vec![open, theirs, s.protected];
    everything.sort_unstable();
    let mut own_and_open = vec![open, s.protected];
    own_and_open.sort_unstable();
    let mut others_view = vec![open, theirs];
    others_view.sort_unstable();

    assert_eq!(
        translations_of(&app, &As::Cookie(&c.subscriber), s.published).await,
        vec![open],
        "neither the draft nor the password-protected member"
    );
    assert_eq!(
        translations_of(&app, &As::Cookie(&c.author), s.published).await,
        own_and_open,
        "another author's draft is not theirs to see"
    );
    assert_eq!(
        translations_of(&app, &As::Cookie(&c.other), s.published).await,
        others_view,
        "the draft's author sees it"
    );
    assert_eq!(
        translations_of(&app, &As::Cookie(&c.editor), s.published).await,
        everything
    );
    assert_eq!(
        translations_of(&app, &As::Key(EDITOR_KEY_RAW), s.published).await,
        vec![open],
        "the key was not granted edit_others"
    );

    // The responses to a save follow the same rule.
    let (status, body) = call(
        &app,
        &As::Cookie(&c.author),
        Method::PUT,
        &format!("/posts/{}", s.published),
        Some(serde_json::json!({ "excerpt": "saved" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let listed: Vec<i64> = body["translations"]
        .as_array()
        .expect("translations")
        .iter()
        .map(|m| m["id"].as_i64().expect("member id"))
        .collect();
    assert!(!listed.contains(&theirs), "{listed:?}");
}
