//! A password-protected post must never be stored by a shared cache.
//!
//! The gate is a cookie. The unlocked body used to go out under the same
//! `public` policy as every other page, with no `Vary`, so a CDN that
//! honoured the headers would keep the body from the first visitor who
//! knew the password and hand it to the next one, who did not. Only the
//! real headers on the real response settle this.

#![allow(clippy::expect_used, clippy::unwrap_used)]

mod common;

use std::io::Write as _;

use serde_json::json;
use sqlx::PgPool;
use vyasa_db::models::Role;
use vyasa_db::repo::NewUser;
use vyasa_testkit::{TestDb, TestServer};

fn http(r: Result<ureq::Response, ureq::Error>) -> ureq::Response {
    match r {
        Ok(v) | Err(ureq::Error::Status(_, v)) => v,
        Err(ureq::Error::Transport(e)) => panic!("transport {e}"),
    }
}

/// A minimal valid theme, so the single template has something to render.
fn blog_package() -> Vec<u8> {
    let manifest = "name = \"prottest\"\nversion = 1\nauthor = \"t\"\nrequired_api = 1\n";
    let tokens = r##"{"version":1,"colors":{"primary":{"light":"#123456"}}}"##;
    let layout = r#"{"index":[{"id":"main","kind":"latest-posts"}],"single":[{"id":"body","kind":"post-content"}],"archive":[{"id":"main","kind":"latest-posts"}],"page":[{"id":"body","kind":"post-content"}],"search":[{"id":"main","kind":"search-box"}],"not-found":[{"id":"nf","kind":"content"}]}"#;
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut w = zip::ZipWriter::new(&mut buf);
        for (name, data) in [
            ("manifest.toml", manifest.as_bytes()),
            ("tokens.json", tokens.as_bytes()),
            ("layout.json", layout.as_bytes()),
        ] {
            w.start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            w.write_all(data).unwrap();
        }
        w.finish().unwrap();
    }
    buf.into_inner()
}

/// A running server with an admin, a theme and a session cookie, on its
/// own private database.
struct Site {
    db: TestDb,
    server: TestServer,
    session: String,
}

impl Site {
    fn base(&self) -> &str {
        self.server.base()
    }

    fn pool(&self) -> &PgPool {
        self.db.pool()
    }
}

async fn seed_admin(pool: &PgPool) {
    let hash = vyasa_core::user::password::hash_password("pw-secret-1").unwrap();
    vyasa_db::repo::UsersRepo::new(pool.clone())
        .insert(&NewUser {
            id: 92_001,
            email: "prot-admin@example.com",
            username: "prot-admin",
            display_name: "Prot Admin",
            password_hash: Some(&hash),
            role: Role::Admin,
            bio: "",
        })
        .await
        .expect("admin");
}

fn login(base: &str) -> String {
    let login = http(
        ureq::post(format!("{base}/api/v1/auth/login").as_str())
            .send_json(json!({"email": "prot-admin@example.com", "password": "pw-secret-1"})),
    );
    assert_eq!(login.status(), 200);
    let token = login
        .all("set-cookie")
        .join("; ")
        .split(';')
        .next()
        .and_then(|kv| kv.split('=').nth(1))
        .unwrap()
        .to_string();
    format!("vy_session={token}")
}

/// A theme, so the post has a template to render through.
fn install_theme(base: &str, session: &str) {
    let bytes = blog_package();
    let boundary = "----vyasaprot";
    let mut body = Vec::new();
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; \
             filename=\"t.vytheme\"\r\nContent-Type: application/zip\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(&bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    let installed = http(
        ureq::post(format!("{base}/api/v1/themes").as_str())
            .set("Cookie", session)
            .set(
                "Content-Type",
                &format!("multipart/form-data; boundary={boundary}"),
            )
            .send_bytes(&body),
    );
    assert_eq!(installed.status(), 201, "theme install");
    let themes: serde_json::Value = http(
        ureq::get(format!("{base}/api/v1/themes").as_str())
            .set("Cookie", session)
            .call(),
    )
    .into_json()
    .unwrap();
    let theme_id = themes[0]["id"].to_string().trim_matches('"').to_owned();
    let activated = http(
        ureq::post(format!("{base}/api/v1/themes/{theme_id}/activate").as_str())
            .set("Cookie", session)
            .call(),
    );
    assert!(activated.status() < 300, "activate: {}", activated.status());
}

async fn site() -> Site {
    let db = TestDb::new().await;
    seed_admin(db.pool()).await;
    let server = TestServer::start(common::BIN, &db);
    let session = login(server.base());
    install_theme(server.base(), &session);
    Site {
        db,
        server,
        session,
    }
}

#[tokio::test]
async fn an_unlocked_protected_post_is_private_and_varies_by_cookie() {
    let site = site().await;
    let (base, session) = (site.base(), &site.session);

    // A published post behind a password.
    let created = http(
        ureq::post(format!("{base}/api/v1/posts").as_str())
            .set("Cookie", session)
            .send_json(json!({
                "title": "Members only",
                "slug": "members-only",
                "status": "published",
                "password": "open-sesame",
                "content": {"schema_version": 1, "blocks": [
                    {"kind": "paragraph", "attrs": {"text": "the secret body"}, "children": []}
                ]},
            })),
    );
    let created_status = created.status();
    let created_body: serde_json::Value = created.into_json().expect("post json");
    assert_eq!(created_status, 201, "{created_body}");
    let post_id = created_body["id"].to_string().trim_matches('"').to_owned();

    // Locked: the gate, and nothing a cache may keep.
    let gate = http(ureq::get(format!("{base}/post/members-only").as_str()).call());
    assert_eq!(gate.status(), 200);
    assert_eq!(
        gate.header("cache-control"),
        Some("private, no-store"),
        "the gate"
    );
    let gate_html = gate.into_string().unwrap();
    assert!(gate_html.contains("password"), "{gate_html}");
    assert!(
        !gate_html.contains("the secret body"),
        "leaked before unlock"
    );

    // Unlock. The server answers with a redirect and the cookie; do not
    // follow it, the cookie is the point.
    let agent = ureq::builder().redirects(0).build();
    let unlocked = http(
        agent
            .post(format!("{base}/post/members-only/password").as_str())
            .send_string("password=open-sesame"),
    );
    assert!(
        (300..400).contains(&unlocked.status()),
        "unlock should redirect, got {}",
        unlocked.status()
    );
    let post_cookie = unlocked
        .all("set-cookie")
        .into_iter()
        .find(|c| c.starts_with("vy_post_"))
        .expect("unlock cookie")
        .split(';')
        .next()
        .unwrap()
        .to_owned();

    // Unlocked: the body, and headers that forbid any shared cache from
    // keeping it or serving it to someone without the cookie.
    let page = http(
        ureq::get(format!("{base}/post/members-only").as_str())
            .set("Cookie", &post_cookie)
            .call(),
    );
    assert_eq!(page.status(), 200);
    assert_eq!(
        page.header("cache-control"),
        Some("private, no-store"),
        "an unlocked body must not be storable by a shared cache"
    );
    assert_eq!(page.header("vary"), Some("Cookie"));
    let html = page.into_string().unwrap();
    assert!(html.contains("the secret body"), "{html}");

    // Comments have a ceiling. There was none: one comment could carry
    // megabytes through the public form, stored and rendered whole.
    let essay = "x".repeat(10_001);
    let too_long = http(
        ureq::post(format!("{base}/api/v1/posts/{post_id}/comments").as_str())
            .set("Cookie", &post_cookie)
            .send_json(json!({
                "author_name": "A", "author_email": "a@example.com", "content": essay
            })),
    );
    assert_eq!(too_long.status(), 400);
    let msg: serde_json::Value = too_long.into_json().unwrap();
    assert!(
        msg["message"].as_str().unwrap_or("").contains("limited to"),
        "{msg}"
    );

    // A protected entry is `noindex` when fetched; listing it in the
    // sitemap only sends a crawler to a page that then refuses indexing.
    let sitemap = http(ureq::get(format!("{base}/sitemap.xml").as_str()).call());
    assert_eq!(sitemap.status(), 200);
    let sitemap = sitemap.into_string().unwrap();
    assert!(
        !sitemap.contains("members-only"),
        "protected post in sitemap: {sitemap}"
    );

    // And an ordinary page keeps the shared policy, so this is not a
    // blanket change that switched caching off for the whole site.
    let home = http(ureq::get(format!("{base}/").as_str()).call());
    assert!(
        home.header("cache-control")
            .unwrap_or("")
            .starts_with("public"),
        "{:?}",
        home.header("cache-control")
    );
}

#[tokio::test]
async fn a_protected_page_unlocks_through_the_shared_endpoint() {
    let site = site().await;
    let (base, session) = (site.base(), &site.session);
    let agent = ureq::builder().redirects(0).build();

    // A protected *page* could never be unlocked: its gate posted to
    // /{slug}/password, which is the GET-only entry route. The gate now
    // posts to one endpoint by id, for every kind of entry.
    let page = http(
        ureq::post(format!("{base}/api/v1/posts").as_str())
            .set("Cookie", session)
            .send_json(json!({
                "type": "page",
                "title": "Secret page",
                "slug": "secret-page",
                "status": "published",
                "password": "open-sesame",
                "content": {"schema_version": 1, "blocks": [
                    {"kind": "paragraph", "attrs": {"text": "the page body"}, "children": []}
                ]},
            })),
    );
    let page_body: serde_json::Value = page.into_json().unwrap();
    let page_id = page_body["id"].to_string().trim_matches('"').to_owned();
    let page_gate = http(ureq::get(format!("{base}/secret-page").as_str()).call());
    assert_eq!(page_gate.status(), 200);
    let gate_html = page_gate.into_string().unwrap();
    assert!(gate_html.contains(r#"action="/unlock""#), "{gate_html}");
    assert!(
        gate_html.contains(&page_id),
        "the form must carry the id: {gate_html}"
    );
    let page_unlocked = http(
        agent
            .post(format!("{base}/unlock").as_str())
            .send_string(&format!("post_id={page_id}&password=open-sesame")),
    );
    assert!(
        (300..400).contains(&page_unlocked.status()),
        "{}",
        page_unlocked.status()
    );
    assert_eq!(page_unlocked.header("location"), Some("/secret-page"));
    let page_cookie = page_unlocked
        .all("set-cookie")
        .into_iter()
        .find(|c| c.starts_with("vy_post_"))
        .expect("unlock cookie")
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let page_view = http(
        ureq::get(format!("{base}/secret-page").as_str())
            .set("Cookie", &page_cookie)
            .call(),
    );
    assert_eq!(page_view.status(), 200);
    assert_eq!(page_view.header("cache-control"), Some("private, no-store"));
    assert!(page_view.into_string().unwrap().contains("the page body"));
}

#[tokio::test]
async fn a_draft_takes_no_comments() {
    let site = site().await;
    let (base, session) = (site.base(), &site.session);

    // And a post that is not on the site takes no comments: a draft is
    // not published, and a comment on it either probes ids or leaks that
    // the draft exists.
    let draft = http(
        ureq::post(format!("{base}/api/v1/posts").as_str())
            .set("Cookie", session)
            .send_json(json!({
                "title": "Not yet",
                "slug": "not-yet",
                "status": "draft",
                "content": {"schema_version": 1, "blocks": []},
            })),
    );
    let draft_body: serde_json::Value = draft.into_json().unwrap();
    let draft_id = draft_body["id"].to_string().trim_matches('"').to_owned();
    let on_draft = http(
        ureq::post(format!("{base}/api/v1/posts/{draft_id}/comments").as_str()).send_json(json!({
            "author_name": "A", "author_email": "a@example.com", "content": "hello?"
        })),
    );
    // To someone who cannot see the draft it is answered exactly as a
    // missing post (phase 99): "comments are closed" said that it exists.
    assert_eq!(on_draft.status(), 404, "a draft took a comment");
    let msg: serde_json::Value = on_draft.into_json().unwrap();
    assert_eq!(msg["code"], "post_not_found", "{msg}");
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn integrated_permalinks_graphql_history_and_menu_cache() {
    let site = site().await;
    let (base, session) = (site.base(), &site.session);
    let set = |key: &str, value: serde_json::Value| {
        let response = http(
            ureq::put(&format!("{base}/api/v1/options/{key}"))
                .set("cookie", session)
                .send_json(value),
        );
        assert_eq!(response.status(), 204);
    };
    set("permalink_pattern", json!("/post/{slug}"));
    set("date_format", json!("%Y-%m-%d"));
    set("timezone", json!("America/Los_Angeles"));
    let created = http(ureq::post(&format!("{base}/api/v1/posts"))
        .set("cookie", session).send_json(json!({
            "title":"Integration original", "slug":"integration-original", "status":"published",
            "content":{"schema_version":1,"blocks":[{"kind":"paragraph","attrs":{"text":"Integration body"}}]}
        })));
    assert_eq!(created.status(), 201);
    let post: serde_json::Value = created.into_json().unwrap();
    let id = post["id"].as_i64().unwrap();
    let gql = http(ureq::post(&format!("{base}/api/graphql"))
        .set("cookie", session).send_json(json!({"query":format!(
            "mutation {{ updatePost(id:{id}, input:{{title:\"Integration changed\", slug:\"integration-changed\"}}) {{ title url }} }}"
        )})));
    let gql: serde_json::Value = gql.into_json().unwrap();
    assert!(gql.get("errors").is_none(), "{gql}");
    assert_eq!(
        gql["data"]["updatePost"]["url"],
        "/post/integration-changed"
    );
    let revisions: serde_json::Value = http(
        ureq::get(&format!("{base}/api/v1/posts/{id}/revisions"))
            .set("cookie", session)
            .call(),
    )
    .into_json()
    .unwrap();
    assert!(revisions
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r["title"] == "Integration changed"));
    let agent = ureq::builder().redirects(0).build();
    let old = http(
        agent
            .get(&format!("{base}/post/integration-original"))
            .call(),
    );
    assert_eq!(old.status(), 301);
    assert_eq!(old.header("location"), Some("/post/integration-changed"));
    set("permalink_pattern", json!("/journal/{year}/{month}/{slug}"));
    let post: serde_json::Value = http(
        ureq::get(&format!("{base}/api/v1/posts/{id}"))
            .set("cookie", session)
            .call(),
    )
    .into_json()
    .unwrap();
    let canonical = post["public_url"].as_str().unwrap();
    assert!(canonical.starts_with("/journal/"));
    let page = http(ureq::get(&format!("{base}{canonical}")).call());
    assert_eq!(page.status(), 200);
    assert!(page.into_string().unwrap().contains("Integration body"));
    for old in ["/post/integration-original", "/post/integration-changed"] {
        let redirected = http(agent.get(&format!("{base}{old}")).call());
        assert_eq!(redirected.status(), 301);
        assert_eq!(redirected.header("location"), Some(canonical));
    }
    for path in ["/feed.xml", "/atom.xml", "/sitemap.xml"] {
        let body = http(ureq::get(&format!("{base}{path}")).call())
            .into_string()
            .unwrap();
        assert!(body.contains(canonical), "{path}: {body}");
    }
    let markdown = http(ureq::get(&format!("{base}{canonical}.md")).call());
    assert_eq!(markdown.status(), 200);
    assert!(markdown.into_string().unwrap().contains("Integration body"));
    // A second change must preserve the previous custom address too.
    set("permalink_pattern", json!("/{slug}"));
    let previous = http(agent.get(&format!("{base}{canonical}")).call());
    assert_eq!(previous.status(), 301);
    assert_eq!(previous.header("location"), Some("/integration-changed"));
    assert_eq!(
        http(ureq::get(&format!("{base}/integration-changed")).call()).status(),
        200
    );

    // Install a navigation section, warm the cache, then edit its menu.
    let pool = site.pool();
    sqlx::query("UPDATE themes SET layout = jsonb_set(layout, '{index}', '[{\"id\":\"nav\",\"kind\":\"menu\",\"settings\":{\"slug\":\"integration-menu\"}},{\"id\":\"main\",\"kind\":\"latest-posts\"}]'::jsonb), version = version + 1 WHERE is_active = true")
        .execute(pool).await.unwrap();
    let menu: serde_json::Value = http(
        ureq::post(&format!("{base}/api/v1/menus"))
            .set("cookie", session)
            .send_json(json!({"name":"Integration menu","slug":"integration-menu"})),
    )
    .into_json()
    .unwrap();
    let before = http(ureq::get(&format!("{base}/")).call());
    let etag = before.header("etag").unwrap().to_owned();
    assert!(!before.into_string().unwrap().contains("New navigation"));
    let item = http(
        ureq::post(&format!("{base}/api/v1/menus/{}/items", menu["id"]))
            .set("cookie", session)
            .send_json(
                json!({"label":"New navigation", "url":"/integration-changed", "sort_order":0}),
            ),
    );
    assert_eq!(item.status(), 201);
    let item: serde_json::Value = item.into_json().unwrap();
    let after = http(
        ureq::get(&format!("{base}/"))
            .set("If-None-Match", &etag)
            .call(),
    );
    assert_eq!(after.status(), 200);
    assert!(after.into_string().unwrap().contains("New navigation"));
    let changed = http(
        ureq::patch(&format!("{base}/api/v1/menus/items/{}", item["id"]))
            .set("cookie", session)
            .send_json(json!({"label":"Edited navigation"})),
    );
    assert_eq!(changed.status(), 200);
    let page = http(ureq::get(&format!("{base}/")).call())
        .into_string()
        .unwrap();
    assert!(page.contains("Edited navigation"));
    assert!(!page.contains("New navigation"));
    let removed = http(
        ureq::delete(&format!("{base}/api/v1/menus/items/{}", item["id"]))
            .set("cookie", session)
            .call(),
    );
    assert_eq!(removed.status(), 204);
    assert!(!http(ureq::get(&format!("{base}/")).call())
        .into_string()
        .unwrap()
        .contains("Edited navigation"));
    set("permalink_pattern", json!("/post/{slug}"));
    set("timezone", json!("UTC"));
}

/// A JSON request with an optional cookie; returns status and body.
fn send(
    method: &str,
    url: &str,
    cookie: Option<&str>,
    body: Option<serde_json::Value>,
) -> (u16, serde_json::Value) {
    let mut req = ureq::request(method, url);
    if let Some(c) = cookie {
        req = req.set("Cookie", c);
    }
    let resp = http(match body {
        Some(b) => req.send_json(b),
        None => req.call(),
    });
    let status = resp.status();
    let text = resp.into_string().unwrap_or_default();
    (
        status,
        serde_json::from_str(&text).unwrap_or(serde_json::Value::Null),
    )
}

fn graphql(base: &str, cookie: Option<&str>, query: &str) -> serde_json::Value {
    send(
        "POST",
        &format!("{base}/api/graphql"),
        cookie,
        Some(json!({ "query": query })),
    )
    .1
}

/// Comments on a published, password-protected entry follow one rule
/// everywhere (`policy::comment_target`): a caller who has not unlocked
/// it and may not edit it is answered exactly as for a missing entry —
/// REST list and create, GraphQL list and submit, and the HTML form. The
/// unlock (the signed token, as `?token=` or the page's unlock cookie)
/// opens it; an editor needs none.
#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn comments_on_a_protected_entry_need_it_unlocked() {
    let site = site().await;
    let (base, session) = (site.base(), &site.session);
    let subscriber = common::seed_user(site.pool(), Role::Subscriber).await;
    let sub = common::login_cookie(base, &subscriber.email, &subscriber.password);
    let (status, created) = send(
        "POST",
        &format!("{base}/api/v1/posts"),
        Some(session),
        Some(json!({
            "title": "Locked", "slug": "locked", "status": "published",
            "password": "open-sesame",
            "content": {"schema_version": 1, "blocks": []},
        })),
    );
    assert_eq!(status, 201, "{created}");
    let id = created["id"].to_string().trim_matches('"').to_owned();
    let missing = "9999997";
    let comment = json!({"author_name": "V", "author_email": "v@example.com", "content": "hi"});
    let bad = json!({"author_name": "", "author_email": "nope", "content": ""});

    // REST, anonymous and as a subscriber: the missing entry's answer,
    // before the body is looked at.
    let (gone, gone_body) = send(
        "GET",
        &format!("{base}/api/v1/posts/{missing}/comments"),
        None,
        None,
    );
    assert_eq!(gone, 404);
    assert_eq!(gone_body["code"], "post_not_found");
    for cookie in [None, Some(sub.as_str())] {
        let (status, body) = send(
            "GET",
            &format!("{base}/api/v1/posts/{id}/comments"),
            cookie,
            None,
        );
        assert_eq!((status, &body["code"]), (404, &gone_body["code"]), "{body}");
        for payload in [&comment, &bad] {
            let (status, body) = send(
                "POST",
                &format!("{base}/api/v1/posts/{id}/comments"),
                cookie,
                Some(payload.clone()),
            );
            assert_eq!((status, &body["code"]), (404, &gone_body["code"]), "{body}");
        }
        // A token that is not the entry's opens nothing.
        let (status, _) = send(
            "GET",
            &format!("{base}/api/v1/posts/{id}/comments?token=forged"),
            cookie,
            None,
        );
        assert_eq!(status, 404);
    }

    // GraphQL: as a missing id.
    let gql_err = |cookie: Option<&str>, query: String| -> String {
        graphql(base, cookie, &query)["errors"][0]["message"]
            .as_str()
            .unwrap_or_default()
            .replace(&id, "ID")
            .replace(missing, "ID")
    };
    let submit = |cookie: Option<&str>, target: &str| {
        gql_err(cookie, format!(
            "mutation {{ submitComment(input: {{postId: {target}, authorName: \"V\", authorEmail: \"v@example.com\", content: \"hi\"}}) {{ id }} }}"
        ))
    };
    let list = |cookie: Option<&str>, target: &str| {
        gql_err(
            cookie,
            format!("{{ comments(postId: {target}) {{ totalCount }} }}"),
        )
    };
    let gone_submit = submit(None, missing);
    let gone_list = list(None, missing);
    assert!(!gone_submit.is_empty() && !gone_list.is_empty());
    for cookie in [None, Some(sub.as_str())] {
        assert_eq!(submit(cookie, &id), gone_submit);
        assert_eq!(list(cookie, &id), gone_list);
    }

    // The HTML form, without the unlock cookie: the not-found page.
    let agent = ureq::builder().redirects(0).build();
    let form = format!("post_id={id}&author_name=V&author_email=v%40example.com&content=hi");
    let html = http(
        agent
            .post(&format!("{base}/comment"))
            .set("Content-Type", "application/x-www-form-urlencoded")
            .send_string(&form),
    );
    assert_eq!(html.status(), 404, "the locked form took a comment");

    // Unlocked: the page's cookie opens the form and REST; the token
    // (from verify-password) opens REST as `?token=`.
    let unlocked = http(
        agent
            .post(&format!("{base}/unlock"))
            .send_string(&format!("post_id={id}&password=open-sesame")),
    );
    assert!((300..400).contains(&unlocked.status()));
    let post_cookie = unlocked
        .all("set-cookie")
        .into_iter()
        .find(|c| c.starts_with("vy_post_"))
        .expect("unlock cookie")
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let html = http(
        agent
            .post(&format!("{base}/comment"))
            .set("Content-Type", "application/x-www-form-urlencoded")
            .set("Cookie", &post_cookie)
            .send_string(&form),
    );
    assert_eq!(html.status(), 303, "the unlocked form takes a comment");
    let (status, body) = send(
        "GET",
        &format!("{base}/api/v1/posts/{id}/comments"),
        Some(&post_cookie),
        None,
    );
    assert_eq!(status, 200, "{body}");
    let (status, token) = send(
        "POST",
        &format!("{base}/api/v1/posts/{id}/verify-password"),
        Some(&sub),
        Some(json!({"password": "open-sesame"})),
    );
    assert_eq!(status, 200, "{token}");
    let token = token["token"].as_str().unwrap().to_owned();
    let (status, body) = send(
        "POST",
        &format!("{base}/api/v1/posts/{id}/comments?token={token}"),
        None,
        Some(comment.clone()),
    );
    assert_eq!(status, 201, "{body}");
    let (status, _) = send(
        "GET",
        &format!("{base}/api/v1/posts/{id}/comments?token={token}"),
        None,
        None,
    );
    assert_eq!(status, 200);

    // An editor needs no unlock, on any surface.
    let (status, body) = send(
        "GET",
        &format!("{base}/api/v1/posts/{id}/comments"),
        Some(session),
        None,
    );
    assert_eq!(status, 200, "{body}");
    let (status, body) = send(
        "POST",
        &format!("{base}/api/v1/posts/{id}/comments"),
        Some(session),
        Some(comment.clone()),
    );
    assert_eq!(status, 201, "{body}");
    let listed = graphql(
        base,
        Some(session),
        &format!("{{ comments(postId: {id}) {{ totalCount }} }}"),
    );
    assert!(listed["errors"].is_null(), "{listed}");
    let submitted = graphql(
        base,
        Some(session),
        &format!(
            "mutation {{ submitComment(input: {{postId: {id}, content: \"from the editor\"}}) {{ id }} }}"
        ),
    );
    assert!(submitted["errors"].is_null(), "{submitted}");
}

/// Someone who can see a draft is not told it is missing: commenting on
/// it is refused for what it is — comments are closed — on REST and
/// GraphQL alike.
#[tokio::test]
async fn an_editor_who_sees_a_draft_is_told_comments_are_closed() {
    let site = site().await;
    let (base, session) = (site.base(), &site.session);
    let (status, created) = send(
        "POST",
        &format!("{base}/api/v1/posts"),
        Some(session),
        Some(json!({
            "title": "Draft", "slug": "a-draft", "status": "draft",
            "content": {"schema_version": 1, "blocks": []},
        })),
    );
    assert_eq!(status, 201, "{created}");
    let id = created["id"].to_string().trim_matches('"').to_owned();
    let (status, body) = send(
        "POST",
        &format!("{base}/api/v1/posts/{id}/comments"),
        Some(session),
        Some(json!({"author_name": "E", "author_email": "e@example.com", "content": "hi"})),
    );
    assert_eq!(status, 400, "{body}");
    assert_eq!(
        body["message"], "Comments are closed on this post.",
        "{body}"
    );
    let out = graphql(
        base,
        Some(session),
        &format!("mutation {{ submitComment(input: {{postId: {id}, content: \"hi\"}}) {{ id }} }}"),
    );
    let message = out["errors"][0]["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("Comments are closed on this post."),
        "{out}"
    );
}
