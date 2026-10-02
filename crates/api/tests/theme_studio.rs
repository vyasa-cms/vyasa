//! The theme studio over REST: a draft starts from the live theme, edits
//! are validated and recorded, previews render the candidate, publishing
//! installs a version, and previews of unpublished work need a session.
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod common;

use serde_json::{json, Value};
use sqlx::PgPool;
use vyasa_db::{models::Role, repo::NewUser, repo::UsersRepo};
use vyasa_testkit::{TestDb, TestServer};

fn http(r: Result<ureq::Response, ureq::Error>) -> ureq::Response {
    match r {
        Ok(v) | Err(ureq::Error::Status(_, v)) => v,
        Err(ureq::Error::Transport(e)) => panic!("transport {e}"),
    }
}

fn body_text(r: ureq::Response) -> String {
    use std::io::Read;
    let mut s = String::new();
    r.into_reader()
        .take(4_000_000)
        .read_to_string(&mut s)
        .unwrap();
    s
}

fn json_body(r: ureq::Response) -> Value {
    serde_json::from_str(&body_text(r)).unwrap()
}

fn login(base: &str, email: &str) -> String {
    let resp = http(
        ureq::post(format!("{base}/api/v1/auth/login").as_str())
            .send_json(json!({"email": email, "password": "pw-secret-1"})),
    );
    assert_eq!(resp.status(), 200, "login {email}");
    let token = resp
        .all("set-cookie")
        .join("; ")
        .split(';')
        .next()
        .and_then(|kv| kv.split('=').nth(1))
        .unwrap()
        .to_string();
    format!("vy_session={token}")
}

async fn seed(pool: &PgPool) {
    let hash = vyasa_core::user::password::hash_password("pw-secret-1").unwrap();
    let users = UsersRepo::new(pool.clone());
    users
        .insert(&NewUser {
            id: 84_001,
            email: "studio-admin@example.com",
            username: "studioadmin",
            display_name: "Studio Admin",
            password_hash: Some(&hash),
            role: Role::Admin,
            bio: "",
        })
        .await
        .expect("admin");
    users
        .insert(&NewUser {
            id: 84_002,
            email: "studio-author@example.com",
            username: "studioauthor",
            display_name: "Studio Author",
            password_hash: Some(&hash),
            role: Role::Author,
            bio: "",
        })
        .await
        .expect("author");
    // One published post so single-page previews have something to show.
    sqlx::query(
        "INSERT INTO posts (id, author_id, type, status, slug, title, content, excerpt, published_at)
         VALUES (84100, 84001, 'post', 'published', 'hello-studio', 'Hello studio',
                 '{\"schema_version\":1,\"blocks\":[{\"kind\":\"paragraph\",\"attrs\":{\"text\":\"Studio body text.\"}}]}',
                 'An excerpt', now())",
    )
    .execute(pool)
    .await
    .expect("post");
}

fn get(base: &str, cookie: &str, path: &str) -> ureq::Response {
    http(
        ureq::get(format!("{base}{path}").as_str())
            .set("Cookie", cookie)
            .call(),
    )
}

fn post(base: &str, cookie: &str, path: &str, body: Value) -> ureq::Response {
    http(
        ureq::post(format!("{base}{path}").as_str())
            .set("Cookie", cookie)
            .send_json(body),
    )
}

#[tokio::test]
async fn draft_lifecycle_over_rest() {
    let db = TestDb::new().await;
    seed(db.pool()).await;
    let server = TestServer::start(common::BIN, &db);
    let base = server.base();
    let admin = login(base, "studio-admin@example.com");
    let author = login(base, "studio-author@example.com");

    let (id, after) = edits_are_validated_and_recorded(base, &admin, &author);
    previews_render_the_candidate(base, &admin, id, &after);
    // Its own draft: the phases around it count revisions and assert an
    // empty conversation, and a proposal adds to both.
    proposals_wait_to_be_accepted(db.pool(), base, &admin).await;
    publish_and_preview_access(base, &admin, &author, id);
    starter_assets_survive_studio(db.pool(), base, &admin).await;
    docs_preview_keeps_headings(db.pool(), base, &admin).await;
}

/// A proposal that changes the theme AND stages a navigation menu —
/// which lands only on accept, through the same MenuService rules as
/// the Menus page.
fn proposal_with_menu(tokens: &Value, layout: &Value) -> Value {
    json!({
        "tokens": tokens,
        "layout": layout,
        "templates": Value::Null,
        "changes": ["tokens: primary", "Created menu \"main-nav\" (3 links)"],
        "menus": [{
            "slug": "main-nav",
            "name": "Main Nav",
            "location": "header",
            "items": [
                {"label": "Home", "url": "/"},
                {"label": "Docs", "url": "/docs",
                 "children": [{"label": "API", "url": "/api-docs"}]}
            ],
        }],
    })
}

/// How many links the test proposal's staged `main-nav` menu holds in
/// the database — `None` until an accept writes it.
async fn staged_menu_links(pg: &sqlx::PgPool) -> Option<i64> {
    let menu_id: Option<i64> = sqlx::query_scalar("SELECT id FROM menus WHERE slug = 'main-nav'")
        .fetch_optional(pg)
        .await
        .expect("query menus");
    let menu_id = menu_id?;
    Some(
        sqlx::query_scalar("SELECT count(*) FROM menu_items WHERE menu_id = $1")
            .bind(menu_id)
            .fetch_one(pg)
            .await
            .expect("count items"),
    )
}

/// The assistant offers; the author decides.
///
/// The run itself needs a registered model, so the proposal is written
/// straight to the table — what is under test is the accepting, which is
/// the only thing that commits.
async fn proposals_wait_to_be_accepted(pg: &sqlx::PgPool, base: &str, admin: &str) {
    let id = json_body(post(base, admin, "/api/v1/themes/drafts", json!({})))["id"]
        .as_i64()
        .expect("draft id");

    let before = json_body(get(base, admin, &format!("/api/v1/themes/drafts/{id}")));
    let base_revision = before["revision"].as_i64().unwrap();
    let tokens = before["tokens"].clone();
    let layout = before["layout"].clone();

    let insert = |mid: i64, proposal_base: i64| {
        let tokens = tokens.clone();
        let layout = layout.clone();
        let pg = pg.clone();
        async move {
            sqlx::query(
                "INSERT INTO theme_draft_messages
                     (id, draft_id, role, text, proposal, proposal_base)
                 VALUES ($1, $2, 'assistant', 'Warmed the accent.', $3, $4)",
            )
            .bind(mid)
            .bind(id)
            .bind(proposal_with_menu(&tokens, &layout))
            .bind(i32::try_from(proposal_base).unwrap())
            .execute(&pg)
            .await
            .expect("insert proposal");
        }
    };

    // A proposal composed against an older draft is refused rather than
    // silently discarding whatever was done in between.
    insert(90_501, base_revision - 1).await;
    let stale = post(
        base,
        admin,
        &format!("/api/v1/themes/drafts/{id}/messages/90501/accept"),
        json!({}),
    );
    assert_eq!(stale.status(), 409, "stale proposal accepted");
    assert_eq!(
        staged_menu_links(pg).await,
        None,
        "a refused proposal wrote its menus"
    );

    // The conversation shows what a reply would change, without the
    // documents themselves.
    insert(90_502, base_revision).await;
    let messages = json_body(get(
        base,
        admin,
        &format!("/api/v1/themes/drafts/{id}/messages"),
    ));
    let offered = messages
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"].as_str() == Some("90502") || m["id"].as_i64() == Some(90_502))
        .expect("the proposal is listed");
    assert_eq!(offered["proposal"]["changes"][0], "tokens: primary");
    assert!(
        offered["proposal"].get("tokens").is_none(),
        "documents leaked"
    );
    assert!(offered["revision"].is_null(), "nothing committed yet");

    // Accepting is what writes.
    let accepted = post(
        base,
        admin,
        &format!("/api/v1/themes/drafts/{id}/messages/90502/accept"),
        json!({}),
    );
    assert_eq!(accepted.status(), 200);
    assert_eq!(
        json_body(accepted)["revision"].as_i64().unwrap(),
        base_revision + 1
    );

    // The staged menu landed with the theme — nested items and all.
    assert_eq!(
        staged_menu_links(pg).await,
        Some(3),
        "all staged links landed on accept, children included"
    );

    // And only once: the proposal is cleared, so a second press cannot
    // rewrite the draft with what it said an hour ago.
    let again = post(
        base,
        admin,
        &format!("/api/v1/themes/drafts/{id}/messages/90502/accept"),
        json!({}),
    );
    assert_eq!(again.status(), 400, "a settled proposal was applied twice");

    let missing = post(
        base,
        admin,
        &format!("/api/v1/themes/drafts/{id}/messages/90599/accept"),
        json!({}),
    );
    assert_eq!(missing.status(), 404);

    let _ = http(
        ureq::delete(format!("{base}/api/v1/themes/drafts/{id}").as_str())
            .set("Cookie", admin)
            .call(),
    );
}

/// Vocabulary, permissions, rejected and accepted edits, history.
fn edits_are_validated_and_recorded(base: &str, admin: &str, author: &str) -> (i64, Value) {
    // Vocabulary comes from the registry: every block kind, every template.
    let vocab = json_body(get(base, admin, "/api/v1/themes/vocabulary"));
    let kinds: Vec<&str> = vocab["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["kind"].as_str().unwrap())
        .collect();
    assert!(kinds.contains(&"latest-posts") && kinds.contains(&"related-posts"));
    assert_eq!(vocab["template_files"].as_array().unwrap().len(), 7);
    assert!(vocab["token_schema"]["properties"]["colors"].is_object());

    // Authors cannot touch themes at all.
    assert_eq!(get(base, author, "/api/v1/themes/drafts").status(), 403);

    // A draft starts from the live theme (bootstrap activated `blog`).
    let created = post(base, admin, "/api/v1/themes/drafts", json!({}));
    assert_eq!(created.status(), 201);
    let draft = json_body(created);
    let id = draft["id"].as_i64().unwrap();
    assert_eq!(draft["name"], "blog (draft)");
    assert_eq!(draft["revision"], 1);
    assert_eq!(draft["status"], "ready");
    assert_eq!(draft["tokens"]["colors"]["primary"]["light"], "#9c3e25");
    assert!(draft["layout"]["index"].is_array());

    // A rejected edit changes nothing and names the field.
    let bad = post(
        base,
        admin,
        &format!("/api/v1/themes/drafts/{id}/ops"),
        json!({"ops": [{"op": "patch_tokens", "patch": {"colors": {"primary": {"light": "orange"}}}}]}),
    );
    assert_eq!(bad.status(), 400);
    let msg = json_body(bad)["message"].as_str().unwrap().to_owned();
    assert!(msg.contains("colors.primary.light"), "{msg}");
    let same = json_body(get(base, admin, &format!("/api/v1/themes/drafts/{id}")));
    assert_eq!(same["revision"], 1);

    // A good edit is a revision with a generated note; a low-contrast
    // colour warns without blocking.
    let ok = post(
        base,
        admin,
        &format!("/api/v1/themes/drafts/{id}/ops"),
        json!({"ops": [
            {"op": "patch_tokens", "patch": {"colors": {"primary": {"light": "#123456"}, "text": {"light": "#dddddd"}}}},
            {"op": "set_layout", "template": "not-found", "blocks": [{"id": "header", "kind": "header"}, {"id": "msg", "kind": "content"}]}
        ]}),
    );
    assert_eq!(ok.status(), 200);
    let after = json_body(ok);
    assert_eq!(after["revision"], 2);
    assert_eq!(after["tokens"]["colors"]["primary"]["light"], "#123456");
    assert!(after["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|w| w["path"] == "colors.text"));

    let revisions = json_body(get(
        base,
        admin,
        &format!("/api/v1/themes/drafts/{id}/revisions"),
    ));
    let notes: Vec<&str> = revisions
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["note"].as_str().unwrap())
        .collect();
    assert_eq!(notes.len(), 2);
    assert!(notes[0].contains("colors.primary.light"), "{notes:?}");
    assert!(notes[0].contains("not-found layout"), "{notes:?}");
    assert_eq!(notes[1], "Started the draft");
    (id, after)
}

/// Template overrides, draft previews on any path, unsaved candidates.
fn previews_render_the_candidate(base: &str, admin: &str, id: i64, after: &Value) {
    // A template override is compiled in the sandbox; a bad one is refused.
    let escape = post(
        base,
        admin,
        &format!("/api/v1/themes/drafts/{id}/ops"),
        json!({"ops": [{"op": "set_template", "name": "single.html", "source": "{{ get_env(name=\"HOME\") }}"}]}),
    );
    assert_eq!(escape.status(), 400);
    let good_tpl = post(
        base,
        admin,
        &format!("/api/v1/themes/drafts/{id}/ops"),
        json!({"ops": [{"op": "set_template", "name": "single.html", "source":
            "{% extends \"base.html\" %}{% block body %}<article class=\"studio-single\">{{ regions.body | safe }}</article>{% endblock %}"}]}),
    );
    assert_eq!(good_tpl.status(), 200);
    assert_eq!(json_body(good_tpl)["revision"], 3);

    // The draft preview renders real content through the override, and
    // carries the edited colour.
    let single = get(
        base,
        admin,
        &format!("/api/v1/themes/drafts/{id}/preview?path=/post/hello-studio"),
    );
    assert_eq!(single.status(), 200);
    assert_eq!(single.header("x-vyasa-preview"), Some("1"));
    let html = body_text(single);
    assert!(html.contains("studio-single"), "override not applied");
    assert!(html.contains("Studio body text."));
    assert!(html.contains("--vy-color-primary: #123456;"));

    // A theme's own stylesheet and script are edited by an op like any
    // other, and the preview links the *draft's* assets rather than the
    // live theme's — otherwise an author would be editing something they
    // could not see until they published it.
    let assets = post(
        base,
        admin,
        &format!("/api/v1/themes/drafts/{id}/ops"),
        json!({"ops": [{"op": "set_assets",
                        "css": ".studio-single{outline:1px solid red}",
                        "js": "window.studioLoaded=true"}]}),
    );
    assert_eq!(assets.status(), 200, "{}", body_text(assets));

    let previewed = body_text(get(
        base,
        admin,
        &format!("/api/v1/themes/drafts/{id}/preview?path=/post/hello-studio"),
    ));
    let href = previewed
        .split("/theme-assets/")
        .nth(1)
        .and_then(|r| r.split('"').next())
        .expect("a theme asset link in the preview");
    assert!(
        std::path::Path::new(href)
            .extension()
            .is_some_and(|e| e == "css"),
        "{href}"
    );
    assert!(
        previewed.contains("/theme-assets/") && previewed.contains("defer"),
        "both halves linked:\n{previewed}"
    );

    // Oversized assets are refused with a message, not stored.
    let before = json_body(get(base, admin, &format!("/api/v1/themes/drafts/{id}")))["revision"]
        .as_i64()
        .expect("a revision");
    let too_big = post(
        base,
        admin,
        &format!("/api/v1/themes/drafts/{id}/ops"),
        json!({"ops": [{"op": "set_assets", "css": "x".repeat(300_000), "js": ""}]}),
    );
    assert_eq!(too_big.status(), 400);
    assert!(body_text(too_big).contains("maximum"));
    // A refused op commits nothing: history records what happened, not
    // what was attempted.
    let after_reject = json_body(get(base, admin, &format!("/api/v1/themes/drafts/{id}")))
        ["revision"]
        .as_i64()
        .expect("a revision");
    assert_eq!(after_reject, before, "a rejected op left a revision behind");

    // Any path resolves; unknown pages fall through to the 404 template.
    let missing = body_text(get(
        base,
        admin,
        &format!("/api/v1/themes/drafts/{id}/preview?path=/no-such-page"),
    ));
    assert!(missing.contains("data-template=\"not-found\""), "{missing}");

    // A candidate renders without being saved.
    let mut candidate_tokens = after["tokens"].clone();
    candidate_tokens["colors"]["primary"]["light"] = json!("#abcdef");
    let cand = post(
        base,
        admin,
        "/api/v1/themes/preview",
        json!({"tokens": candidate_tokens, "layout": after["layout"], "path": "/"}),
    );
    assert_eq!(cand.status(), 200);
    assert!(body_text(cand).contains("--vy-color-primary: #abcdef;"));
    // A candidate is rendered, not saved: the draft is still on the
    // revision its last op produced.
    let still = json_body(get(base, admin, &format!("/api/v1/themes/drafts/{id}")));
    assert_eq!(still["revision"], 4);

    // A broken candidate explains itself instead of a blank 500.
    let broken = post(
        base,
        admin,
        "/api/v1/themes/preview",
        json!({"tokens": after["tokens"], "layout": after["layout"],
               "templates": {"index.html": "{% extends \"nope.html\" %}"}}),
    );
    assert_eq!(broken.status(), 422);
    assert!(body_text(broken).contains("nope.html"));
}

/// Revert, publish (with and without activate), delete guards, and who
/// may see previews of unpublished work.
fn publish_and_preview_access(base: &str, admin: &str, author: &str, id: i64) {
    // Going back is itself a revision.
    let reverted = post(
        base,
        admin,
        &format!("/api/v1/themes/drafts/{id}/revert"),
        json!({"seq": 1}),
    );
    assert_eq!(reverted.status(), 200);
    let reverted = json_body(reverted);
    // Revisions so far: start, tokens, template, assets — and going back
    // is itself the fifth, because history is append-only.
    assert_eq!(reverted["revision"], 5);
    assert_eq!(reverted["tokens"]["colors"]["primary"]["light"], "#9c3e25");
    // Back to the starter's own single.tera override, not the studio's.
    assert!(reverted["templates"]["single.html"]
        .as_str()
        .unwrap()
        .contains("blog-single"));

    // Publish installs the next version of a name; the draft survives.
    let bad_name = post(
        base,
        admin,
        &format!("/api/v1/themes/drafts/{id}/publish"),
        json!({"name": "Not A Slug"}),
    );
    assert_eq!(bad_name.status(), 400);
    // The next version after whatever blog is currently at, rather than a
    // literal: the rule under test is "publishing over an existing theme
    // increments", and pinning the number made every starter release
    // break this test for no reason.
    let installed = json_body(get(base, admin, "/api/v1/themes"));
    let blog_max = installed
        .as_array()
        .expect("themes list")
        .iter()
        .filter(|t| t["name"] == "blog")
        .filter_map(|t| t["version"].as_i64())
        .max()
        .expect("blog is installed");
    let published = post(
        base,
        admin,
        &format!("/api/v1/themes/drafts/{id}/publish"),
        json!({"name": "blog", "activate": false}),
    );
    assert_eq!(published.status(), 201);
    let theme = json_body(published);
    assert_eq!(theme["name"], "blog");
    assert_eq!(theme["version"], blog_max + 1);
    assert_eq!(theme["is_active"], false);
    let theme_id = theme["id"].as_i64().unwrap();

    // Publishing with activate makes it live and the old version deletable.
    let live = post(
        base,
        admin,
        &format!("/api/v1/themes/drafts/{id}/publish"),
        json!({"name": "studio-out", "activate": true}),
    );
    assert_eq!(live.status(), 201);
    let live = json_body(live);
    assert_eq!(live["version"], 1);
    let themes = json_body(get(base, admin, "/api/v1/themes"));
    let active: Vec<&str> = themes
        .as_array()
        .unwrap()
        .iter()
        .filter(|t| t["is_active"] == true)
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(active, vec!["studio-out"]);

    // The live theme refuses deletion; an inactive version goes.
    let live_id = live["id"].as_i64().unwrap();
    let refused = http(
        ureq::delete(format!("{base}/api/v1/themes/{live_id}").as_str())
            .set("Cookie", admin)
            .call(),
    );
    assert_eq!(refused.status(), 409);
    let gone = http(
        ureq::delete(format!("{base}/api/v1/themes/{theme_id}").as_str())
            .set("Cookie", admin)
            .call(),
    );
    assert_eq!(gone.status(), 204);

    // Installed-theme previews need a session with ManageThemes: anyone
    // else sees the same 404 an unknown path gives.
    let anon = http(ureq::get(format!("{base}/preview/theme/{live_id}").as_str()).call());
    assert_eq!(anon.status(), 404);
    let as_author = http(
        ureq::get(format!("{base}/preview/theme/{live_id}").as_str())
            .set("Cookie", author)
            .call(),
    );
    assert_eq!(as_author.status(), 404);
    let as_admin = http(
        ureq::get(format!("{base}/preview/theme/{live_id}?path=/post/hello-studio").as_str())
            .set("Cookie", admin)
            .call(),
    );
    assert_eq!(as_admin.status(), 200);
    assert!(body_text(as_admin).contains("Studio body text."));

    assistant_and_cleanup(base, admin, id);
}

/// The assistant's guard rails, and discarding a draft.
fn assistant_and_cleanup(base: &str, admin: &str, id: i64) {
    // The assistant refuses up front when no text model is registered,
    // with instructions rather than a failed job later.
    let no_model = post(
        base,
        admin,
        &format!("/api/v1/themes/drafts/{id}/chat"),
        json!({"message": "make it blue"}),
    );
    assert_eq!(no_model.status(), 400);
    let msg = json_body(no_model)["message"].as_str().unwrap().to_owned();
    assert!(msg.to_lowercase().contains("model"), "{msg}");
    let empty = post(
        base,
        admin,
        &format!("/api/v1/themes/drafts/{id}/chat"),
        json!({"message": "   "}),
    );
    assert_eq!(empty.status(), 400);
    let untouched = json_body(get(base, admin, &format!("/api/v1/themes/drafts/{id}")));
    assert_eq!(untouched["status"], "ready");
    assert!(json_body(get(
        base,
        admin,
        &format!("/api/v1/themes/drafts/{id}/messages")
    ))
    .as_array()
    .unwrap()
    .is_empty());

    // Drafts can be discarded.
    let del = http(
        ureq::delete(format!("{base}/api/v1/themes/drafts/{id}").as_str())
            .set("Cookie", admin)
            .call(),
    );
    assert_eq!(del.status(), 204);
    assert_eq!(
        get(base, admin, &format!("/api/v1/themes/drafts/{id}")).status(),
        404
    );
}

async fn starter_assets_survive_studio(pg: &sqlx::PgPool, base: &str, admin: &str) {
    let theme_id: i64 = sqlx::query_scalar(
        "SELECT id FROM themes WHERE name = 'portfolio' ORDER BY version DESC LIMIT 1",
    )
    .fetch_one(pg)
    .await
    .unwrap();
    let created = json_body(post(
        base,
        admin,
        "/api/v1/themes/drafts",
        json!({"base_theme_id": theme_id}),
    ));
    let draft_id = created["id"].as_i64().unwrap();
    assert!(created["assets"]["css"]
        .as_str()
        .unwrap()
        .contains("studio portfolio"));
    let css =
        ".vy { background: url(/theme-assets/images/hero.svg); mask-image: url(images/hero.svg); }";
    let candidate = post(
        base,
        admin,
        "/api/v1/themes/preview",
        json!({
            "tokens": created["tokens"], "layout": created["layout"],
            "templates": created["templates"], "base_theme_id": theme_id.to_string(),
            "assets": {"css": css, "js": "window.previewOnly=true;"}, "path": "/"
        }),
    );
    assert_eq!(candidate.status(), 200);
    let html = body_text(candidate);
    let prefix = format!("/theme-assets/version/{theme_id}/images/hero.svg");
    assert!(html.contains(&prefix));
    let image = get(base, admin, &prefix);
    assert_eq!(image.status(), 200);
    assert!(body_text(image).contains("Modular shapes"));
    let anon = http(ureq::get(format!("{base}{prefix}").as_str()).call());
    assert_eq!(anon.status(), 404);
    preview_asset_access(base, admin, &html, &prefix);
    // A genuine CSS proposal survives acceptance, then publishing.
    let mut proposal = proposal_with_menu(&created["tokens"], &created["layout"]);
    proposal["assets"] = json!({"css": css, "js": ""});
    proposal.as_object_mut().unwrap().remove("menus");
    sqlx::query("INSERT INTO theme_draft_messages (id, draft_id, role, text, proposal, proposal_base) VALUES (990501, $1, 'assistant', 'Updated styles', $2, $3)")
        .bind(draft_id).bind(proposal).bind(i32::try_from(created["revision"].as_i64().unwrap()).unwrap()).execute(pg).await.unwrap();
    let accepted = post(
        base,
        admin,
        &format!("/api/v1/themes/drafts/{draft_id}/messages/990501/accept"),
        json!({}),
    );
    assert_eq!(accepted.status(), 200, "{}", body_text(accepted));
    let current = json_body(get(
        base,
        admin,
        &format!("/api/v1/themes/drafts/{draft_id}"),
    ));
    assert_eq!(current["assets"]["css"], css);
    let published = post(
        base,
        admin,
        &format!("/api/v1/themes/drafts/{draft_id}/publish"),
        json!({"name": "modern-portfolio", "activate": false}),
    );
    assert_eq!(published.status(), 201);
    let published = json_body(published);
    let version = published["id"].as_i64().unwrap();
    let restored = json_body(post(
        base,
        admin,
        "/api/v1/themes/drafts",
        json!({"base_theme_id": version}),
    ));
    assert_eq!(restored["assets"]["css"], css);
    let copied = get(
        base,
        admin,
        &format!("/theme-assets/version/{version}/images/hero.svg"),
    );
    assert_eq!(copied.status(), 200);
    assert!(body_text(copied).contains("Modular shapes"));
}

async fn docs_preview_keeps_headings(pg: &sqlx::PgPool, base: &str, admin: &str) {
    sqlx::query("INSERT INTO posts (id, author_id, type, status, slug, title, content, published_at) VALUES (84101, 84001, 'page', 'published', 'studio-guide', 'Studio guide', $1, now())")
        .bind(json!({"schema_version": 1, "blocks": [{"kind": "heading", "attrs": {"level": 2, "text": "Getting started"}}]}))
        .execute(pg).await.unwrap();
    let theme_id: i64 = sqlx::query_scalar(
        "SELECT id FROM themes WHERE name = 'docs' ORDER BY version DESC LIMIT 1",
    )
    .fetch_one(pg)
    .await
    .unwrap();
    let preview = get(
        base,
        admin,
        &format!("/preview/theme/{theme_id}?path=/studio-guide"),
    );
    assert_eq!(preview.status(), 200);
    let html = body_text(preview);
    assert!(
        html.contains("class=\"vy-toc\""),
        "preview includes the table of contents"
    );
    assert!(
        html.contains("href=\"#vy-h-1\""),
        "table of contents links to the heading"
    );
    assert!(
        html.contains("id=\"vy-h-1\""),
        "heading is an actual anchor"
    );
}

fn preview_asset_access(base: &str, admin: &str, html: &str, prefix: &str) {
    for suffix in ["css", "js"] {
        let path = html
            .split('"')
            .find(|s| s.starts_with("/theme-assets/") && s.ends_with(suffix))
            .unwrap();
        let response = get(base, admin, path);
        assert_eq!(response.status(), 200);
        assert_eq!(response.header("cache-control"), Some("private, no-store"));
        let content = body_text(response);
        if suffix == "css" {
            assert!(content.contains(prefix));
            assert!(content.contains("url(images/hero.svg)"));
            let relative = format!("{}/images/hero.svg", path.rsplit_once('/').unwrap().0);
            assert_eq!(
                get(base, admin, &relative).status(),
                200,
                "relative CSS URLs use the preview version"
            );
        } else {
            assert_eq!(content, "window.previewOnly=true;");
        }
        assert_eq!(
            http(ureq::get(format!("{base}{path}").as_str()).call()).status(),
            404
        );
    }
}
