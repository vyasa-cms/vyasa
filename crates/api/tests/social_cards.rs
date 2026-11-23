//! Social cards on a running server: the head tags of a docs page and the
//! `/_card/{id}-{key}.png` route, with the starter docs theme's layout.
//!
//! The render cache, single flight and render limit are unit-tested in
//! `crates/api/src/social_card.rs`; this file checks what a crawler sees.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::too_many_lines)]

mod common;

use serde_json::json;
use sqlx::PgPool;
use vyasa_db::models::Role;
use vyasa_db::repo::NewUser;
use vyasa_testkit::{TestDb, TestServer};

const DOCS_WASM: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../plugins/docs/docs-component.wasm"
);

const ADMIN: i64 = 89_001;
const PUBLISHED: i64 = 89_101;
const CHILD: i64 = 89_102;
const DRAFT: i64 = 89_103;
const PROTECTED: i64 = 89_104;
const OWN_IMAGE: i64 = 89_105;
const NOINDEX: i64 = 89_106;
const QUOTED: i64 = 89_107;

fn http(r: Result<ureq::Response, ureq::Error>) -> ureq::Response {
    match r {
        Ok(v) | Err(ureq::Error::Status(_, v)) => v,
        Err(ureq::Error::Transport(e)) => panic!("transport {e}"),
    }
}

fn agent() -> ureq::Agent {
    ureq::builder().redirects(0).build()
}

fn get(base: &str, path: &str) -> ureq::Response {
    http(agent().get(&format!("{base}{path}")).call())
}

fn page(base: &str, path: &str) -> String {
    let r = get(base, path);
    assert_eq!(r.status(), 200, "{path}");
    r.into_string().unwrap()
}

fn body(r: ureq::Response) -> Vec<u8> {
    use std::io::Read as _;
    let mut out = Vec::new();
    r.into_reader().read_to_end(&mut out).unwrap();
    out
}

/// The `content` of `<meta property="{prop}">`, if the head has one.
fn meta(html: &str, prop: &str) -> Option<String> {
    let tag = format!("<meta property=\"{prop}\" content=\"");
    let start = html.find(&tag)? + tag.len();
    let end = html[start..].find('"')?;
    Some(html[start..start + end].to_owned())
}

/// The card path in a page's `og:image`.
fn card_path(html: &str) -> String {
    let url = meta(html, "og:image").expect("og:image");
    let at = url.find("/_card/").expect("a card URL");
    url[at..].to_owned()
}

fn doc_row(id: i64, parent: Option<i64>, slug: &str, path: &str, title: &str) -> String {
    format!(
        "({id}, {ADMIN}, 'doc', 'published', '{slug}', '{title}', \
         '{{\"schema_version\":1,\"blocks\":[]}}', '{{}}', {parent}, '{path}', {id}, now())",
        parent = parent.map_or_else(|| "NULL".to_owned(), |p| p.to_string())
    )
}

async fn seed(pool: &PgPool) {
    let hash = vyasa_core::user::password::hash_password("pw-secret-1").unwrap();
    vyasa_db::repo::UsersRepo::new(pool.clone())
        .insert(&NewUser {
            id: ADMIN,
            email: "card-admin@example.com",
            username: "cardadmin",
            display_name: "Card Admin",
            password_hash: Some(&hash),
            role: Role::Admin,
            bio: "",
        })
        .await
        .expect("admin");
    let wasm = std::fs::read(DOCS_WASM).expect("docs plugin fixture");
    let repo = vyasa_db::repo::PluginsRepo::new(pool.clone());
    let row = repo
        .create(
            "vyasa-docs",
            "0.1.0",
            &wasm,
            &vyasa_plugins::host::sha256_hex(&wasm),
            &json!([
                "log:write",
                "kv:read",
                "kv:write",
                "entries:sync",
                "db:publish:posts"
            ]),
        )
        .await
        .expect("install plugin");
    repo.set_enabled(row.id, true).await.expect("enable");
    let rows = [
        doc_row(PUBLISHED, None, "guides", "guides", "Guides"),
        doc_row(
            CHILD,
            Some(PUBLISHED),
            "install",
            "guides/install",
            "Install Krishiv",
        ),
        doc_row(DRAFT, None, "draft", "draft", "Draft page"),
        doc_row(PROTECTED, None, "members", "members", "Members"),
        doc_row(OWN_IMAGE, None, "pictured", "pictured", "Pictured"),
        doc_row(NOINDEX, None, "hidden", "hidden", "Hidden from search"),
        doc_row(QUOTED, None, "quoted", "quoted", "Say \"hi\" <b>"),
    ];
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "INSERT INTO posts (id, author_id, type, status, slug, title, content, meta, parent_id, path, position, published_at) VALUES {}",
        rows.join(",")
    )))
    .execute(pool)
    .await
    .expect("docs");
    for sql in [
        format!("UPDATE posts SET status = 'draft' WHERE id = {DRAFT}"),
        format!("UPDATE posts SET password_hash = '{hash}' WHERE id = {PROTECTED}"),
        format!(
            "UPDATE posts SET meta = '{{\"og_image\":\"https://img.example/own.png\"}}' WHERE id = {OWN_IMAGE}"
        ),
        format!("UPDATE posts SET meta = '{{\"seo_noindex\":true}}' WHERE id = {NOINDEX}"),
    ] {
        sqlx::query(sqlx::AssertSqlSafe(sql))
            .execute(pool)
            .await
            .expect("fixture");
    }
}

fn login(base: &str) -> String {
    let login = http(
        ureq::post(&format!("{base}/api/v1/auth/login"))
            .send_json(json!({"email": "card-admin@example.com", "password": "pw-secret-1"})),
    );
    assert_eq!(login.status(), 200);
    let token = login
        .all("set-cookie")
        .join("; ")
        .split(';')
        .next()
        .and_then(|kv| kv.split('=').nth(1))
        .unwrap()
        .to_owned();
    format!("vy_session={token}")
}

fn activate_docs_theme(base: &str, cookie: &str) {
    let themes: serde_json::Value = http(
        ureq::get(&format!("{base}/api/v1/themes"))
            .set("Cookie", cookie)
            .call(),
    )
    .into_json()
    .unwrap();
    let id = themes
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "docs")
        .map(|t| t["id"].to_string().trim_matches('"').to_owned())
        .expect("the docs starter is installed at boot");
    let activated = http(
        ureq::post(&format!("{base}/api/v1/themes/{id}/activate"))
            .set("Cookie", cookie)
            .call(),
    );
    assert!(activated.status() < 300, "activate: {}", activated.status());
}

fn set_options(base: &str, cookie: &str, options: &serde_json::Value) {
    let r = http(
        ureq::put(&format!("{base}/api/v1/options"))
            .set("Cookie", cookie)
            .send_json(options.clone()),
    );
    assert!(r.status() < 300, "options: {}", r.status());
}

#[tokio::test]
async fn a_docs_page_gets_a_card_and_the_route_serves_it() {
    let db = TestDb::new().await;
    seed(db.pool()).await;
    let server = TestServer::start(common::BIN, &db);
    let base = server.base();
    let cookie = login(base);
    activate_docs_theme(base, &cookie);
    set_options(
        base,
        &cookie,
        &json!({"site_url": "https://docs.example", "site_title": "Card Docs", "social_cards": true}),
    );

    // The status endpoint the admin toggle reads.
    let status: serde_json::Value = http(
        ureq::get(&format!("{base}/api/v1/social-cards"))
            .set("Cookie", &cookie)
            .call(),
    )
    .into_json()
    .unwrap();
    assert_eq!(status["available"], true, "{status}");
    assert_eq!(status["post_types"], json!(["doc"]), "{status}");

    // Head: the card URL and its three tags, each on its own line.
    let html = page(base, "/docs/guides/install");
    let path = card_path(&html);
    assert!(path.starts_with(&format!("/_card/{CHILD}-")), "{path}");
    assert!(html.contains("\n<meta property=\"og:image:width\" content=\"1200\">"));
    assert!(html.contains("\n<meta property=\"og:image:height\" content=\"630\">"));
    assert_eq!(
        meta(&html, "og:image:alt").as_deref(),
        Some("Install Krishiv: Card Docs"),
        "alt from the theme pattern"
    );

    // The route: 200, a 1200x630 PNG, a quoted ETag, a day at the edge.
    let first = get(base, &path);
    assert_eq!(first.status(), 200);
    assert_eq!(first.header("content-type"), Some("image/png"));
    let etag = first.header("etag").unwrap().to_owned();
    assert!(etag.starts_with('"') && etag.ends_with('"'), "{etag}");
    let cache = first.header("cache-control").unwrap().to_owned();
    assert!(
        cache.contains("s-maxage=86400") && cache.contains("immutable"),
        "{cache}"
    );
    let png = body(first);
    assert_eq!(
        vyasa_core::social_card::png_dimensions(&png),
        Some((1200, 630))
    );

    // A second fetch is the same bytes; a matching If-None-Match is a 304.
    assert_eq!(body(get(base, &path)), png);
    let not_modified = http(
        agent()
            .get(&format!("{base}{path}"))
            .set("If-None-Match", &etag)
            .call(),
    );
    assert_eq!(not_modified.status(), 304);

    // An old key on a visible entry: a short redirect to the current URL.
    let stale = format!("/_card/{CHILD}-0000000000000000.png");
    let moved = get(base, &stale);
    assert_eq!(moved.status(), 302);
    assert_eq!(moved.header("location"), Some(path.as_str()));
    let moved_cache = moved.header("cache-control").unwrap();
    assert!(!moved_cache.contains("immutable"), "{moved_cache}");

    // Malformed paths and entries with no card: 404.
    for bad in [
        format!("/_card/{CHILD}.png"),
        format!("/_card/0{CHILD}-0000000000000000.png"),
        format!("/_card/{CHILD}-00000000000000GG.png"),
        "/_card/nope".to_owned(),
    ] {
        assert_eq!(get(base, &bad).status(), 404, "{bad}");
    }
    for id in [DRAFT, PROTECTED, OWN_IMAGE] {
        assert_eq!(
            get(base, &format!("/_card/{id}-0000000000000000.png")).status(),
            404,
            "entry {id}"
        );
    }

    // `seo_noindex` keeps its card; the own image keeps today's tags only.
    let hidden = page(base, "/docs/hidden");
    let hidden_card = card_path(&hidden);
    assert_eq!(get(base, &hidden_card).status(), 200);
    let pictured = page(base, "/docs/pictured");
    assert_eq!(
        meta(&pictured, "og:image").as_deref(),
        Some("https://img.example/own.png")
    );
    assert!(meta(&pictured, "og:image:width").is_none());
    assert!(meta(&pictured, "og:image:alt").is_none());

    // Alt text is escaped.
    let quoted = page(base, "/docs/quoted");
    assert!(
        quoted.contains("content=\"Say &quot;hi&quot; &lt;b&gt;: Card Docs\""),
        "{}",
        meta(&quoted, "og:image:alt").unwrap_or_default()
    );

    // Unpublished after a fetch: 404, the cache is not consulted first.
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "UPDATE posts SET status = 'draft' WHERE id = {NOINDEX}"
    )))
    .execute(db.pool())
    .await
    .unwrap();
    assert_eq!(get(base, &hidden_card).status(), 404);

    // Setting off: no card in the head, 404 on the route.
    set_options(base, &cookie, &json!({"social_cards": false}));
    let off = page(base, "/docs/guides/install");
    assert!(meta(&off, "og:image:width").is_none(), "no card tags");
    assert_eq!(get(base, &path).status(), 404);
}

#[tokio::test]
async fn no_site_url_means_no_image_tags() {
    let db = TestDb::new().await;
    seed(db.pool()).await;
    let server = TestServer::start(common::BIN, &db);
    let base = server.base();
    let cookie = login(base);
    activate_docs_theme(base, &cookie);
    set_options(base, &cookie, &json!({"social_cards": true}));
    let html = page(base, "/docs/guides/install");
    assert!(
        meta(&html, "og:image").is_none(),
        "no og:image without a site URL"
    );
    assert!(meta(&html, "og:image:width").is_none());
}
