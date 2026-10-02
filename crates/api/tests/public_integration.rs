//! What a visitor actually receives: a comment they can post, an author
//! byline, taxonomy links, working pagination, and a `<head>` search
//! engines and social networks can read.
//!
//! These exercise the rendered HTML, not the JSON API — the gap this
//! suite exists to close is precisely that the two had drifted apart.
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod common;

use std::io::Write as _;

use sqlx::PgPool;
use vyasa_db::models::Role;
use vyasa_db::repo::NewUser;
use vyasa_testkit::{TestDb, TestServer};

/// Posts a form exactly as a browser would, without following the
/// redirect — the redirect is what is under test.
fn post_form(base: &str, body: &str) -> ureq::Response {
    let agent = ureq::builder().redirects(0).build();
    http(
        agent
            .post(format!("{base}/comment").as_str())
            .set("Content-Type", "application/x-www-form-urlencoded")
            .send_string(body),
    )
}

fn http(r: Result<ureq::Response, ureq::Error>) -> ureq::Response {
    match r {
        Ok(v) | Err(ureq::Error::Status(_, v)) => v,
        Err(ureq::Error::Transport(e)) => panic!("transport {e}"),
    }
}

fn body_text(r: ureq::Response) -> String {
    use std::io::Read as _;
    let mut s = String::new();
    r.into_reader()
        .take(4_000_000)
        .read_to_string(&mut s)
        .unwrap();
    s
}

fn get(base: &str, path: &str) -> String {
    // Tera escapes `/` as `&#x2F;` inside attributes, which is correct and
    // invisible to a browser but awkward to assert against.
    body_text(http(ureq::get(format!("{base}{path}").as_str()).call())).replace("&#x2F;", "/")
}

/// A theme whose single view carries comments, so the form is reachable.
fn theme_package() -> Vec<u8> {
    let manifest = "name = \"inttest\"\nversion = 1\nauthor = \"t\"\nrequired_api = 1\n";
    let tokens = r##"{"version":1,"colors":{"primary":{"light":"#123456"}}}"##;
    let layout = r#"{
      "index":[{"id":"masthead","kind":"header"},{"id":"nav","kind":"menu"},
               {"id":"posts","kind":"latest-posts"},{"id":"colophon","kind":"footer"}],
      "single":[{"id":"masthead","kind":"header"},{"id":"body","kind":"post-content"},
                {"id":"discussion","kind":"comments"},{"id":"colophon","kind":"footer"}],
      "archive":[{"id":"masthead","kind":"header"},{"id":"list","kind":"latest-posts"}],
      "page":[{"id":"body","kind":"post-content"}],
      "search":[{"id":"find","kind":"search-box"}],
      "not-found":[{"id":"masthead","kind":"header"},{"id":"nf","kind":"content"}]}"#;
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

async fn seed(pool: &PgPool) -> i64 {
    let hash = vyasa_core::user::password::hash_password("pw-secret-1").unwrap();
    UsersRepoAlias::new(pool.clone())
        .insert(&NewUser {
            id: 86_001,
            email: "pub-admin@example.com",
            username: "pubadmin",
            display_name: "Grace Hopper",
            password_hash: Some(&hash),
            role: Role::Admin,
            bio: "",
        })
        .await
        .expect("admin");
    sqlx::query("INSERT INTO options (key, value) VALUES ('site_url', '\"https://example.test\"')")
        .execute(pool)
        .await
        .expect("site_url");
    // Two posts: enough that a page size of one produces a second page.
    for (id, slug, title) in [
        (86_100_i64, "first-entry", "First entry"),
        (86_101, "second-entry", "Second entry"),
    ] {
        sqlx::query(
            "INSERT INTO posts (id, author_id, type, status, slug, title, content, meta, published_at)
             VALUES ($1, 86001, 'post', 'published', $2, $3,
                     '{\"schema_version\":1,\"blocks\":[{\"kind\":\"paragraph\",\"attrs\":{\"text\":\"Body prose for the entry, long enough to become an excerpt on its own.\"}}]}',
                     '{}', now())",
        )
        .bind(id)
        .bind(slug)
        .bind(title)
        .execute(pool)
        .await
        .expect("post");
    }
    // A reusable block pattern must never reach a feed or the sitemap.
    sqlx::query(
        "INSERT INTO posts (id, author_id, type, status, slug, title, content, meta)
         VALUES (86200, 86001, 'block', 'published', 'a-pattern', 'A pattern', '{\"blocks\":[]}', '{}')",
    )
    .execute(pool)
    .await
    .expect("pattern");
    // The entry carries an SEO description written by the editor.
    sqlx::query(
        "UPDATE posts SET meta = '{\"seo_description\":\"A hand-written summary.\"}'
         WHERE id = 86100",
    )
    .execute(pool)
    .await
    .expect("meta");
    // One category, on the first entry.
    sqlx::query(
        "INSERT INTO terms (id, taxonomy, name, slug) VALUES (86300, 'category', 'Rust', 'rust')",
    )
    .execute(pool)
    .await
    .expect("term");
    sqlx::query("INSERT INTO term_relationships (post_id, term_id) VALUES (86100, 86300)")
        .execute(pool)
        .await
        .expect("term rel");
    86_100
}

type UsersRepoAlias = vyasa_db::repo::UsersRepo;

fn login(base: &str) -> String {
    let resp = http(
        ureq::post(format!("{base}/api/v1/auth/login").as_str()).send_json(
            serde_json::json!({"email":"pub-admin@example.com","password":"pw-secret-1"}),
        ),
    );
    assert_eq!(resp.status(), 200);
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

fn install_and_activate(base: &str, cookie: &str) {
    let bytes = theme_package();
    let boundary = "----vyasaint";
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
    let resp = http(
        ureq::post(format!("{base}/api/v1/themes").as_str())
            .set("Cookie", cookie)
            .set(
                "Content-Type",
                &format!("multipart/form-data; boundary={boundary}"),
            )
            .send_bytes(&body),
    );
    assert_eq!(resp.status(), 201, "install: {}", body_text(resp));
    let themes: serde_json::Value =
        serde_json::from_str(&get_auth(base, cookie, "/api/v1/themes")).unwrap();
    let id = themes
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "inttest")
        .map(|t| t["id"].as_i64().unwrap())
        .expect("installed");
    let resp = http(
        ureq::post(format!("{base}/api/v1/themes/{id}/activate").as_str())
            .set("Cookie", cookie)
            .send_string(""),
    );
    assert_eq!(resp.status(), 200);
}

fn get_auth(base: &str, cookie: &str, path: &str) -> String {
    body_text(http(
        ureq::get(format!("{base}{path}").as_str())
            .set("Cookie", cookie)
            .call(),
    ))
}

#[tokio::test]
async fn a_visitor_can_read_and_comment() {
    let db = TestDb::new().await;
    let post_id = seed(db.pool()).await;
    let server = TestServer::start(common::BIN, &db);
    let base = server.base();
    let cookie = login(base);
    install_and_activate(base, &cookie);

    let entry = get(base, "/post/first-entry");
    byline_terms_and_head(&entry);
    let comment_id = commenting_works(base, post_id);
    moderation_is_honoured(base, &cookie, comment_id);
    listings_and_feeds(base);
}

/// The byline, the taxonomy links and the `<head>` the admin has always
/// been able to fill in but no visitor ever saw.
fn byline_terms_and_head(entry: &str) {
    assert!(
        entry.contains("Grace Hopper"),
        "author name missing from the byline: {entry}"
    );
    assert!(!entry.contains("<p class=\"vy-meta\"> · "), "empty byline");
    assert!(
        entry.contains("<time datetime=\""),
        "no machine-readable date"
    );
    // The category assigned in the admin links to its archive.
    assert!(entry.contains("href=\"/category/rust\""), "no term link");
    assert!(entry.contains(">Rust<"), "term name missing");
    // The editor's Search-preview description reaches the page.
    assert!(
        entry.contains("<meta name=\"description\" content=\"A hand-written summary.\">"),
        "seo_description not rendered"
    );
    assert!(
        entry.contains("<link rel=\"canonical\" href=\"https://example.test/post/first-entry\">"),
        "no canonical"
    );
    assert!(entry.contains("property=\"og:title\""), "no open graph");
    assert!(entry.contains("name=\"twitter:card\""), "no twitter card");
    assert!(
        entry.contains("<link rel=\"alternate\" type=\"application/rss+xml\""),
        "feed not advertised"
    );
    assert!(
        entry.contains("article:published_time"),
        "no publication time"
    );
}

/// The whole point: a form, a POST that works, and the comment appearing.
fn commenting_works(base: &str, post_id: i64) -> i64 {
    let entry = get(base, "/post/first-entry");
    assert!(
        entry.contains("<form class=\"vy-comment-form\""),
        "no comment form on the page"
    );
    assert!(entry.contains("action=\"/comment\""), "form has no action");
    assert!(entry.contains("name=\"author_name\""), "no name field");
    assert!(entry.contains("Be the first to comment."), "no empty state");

    // Posting exactly what a browser sends.
    let resp = post_form(
        base,
        &format!(
            "post_id={post_id}&author_name=Ada+Lovelace\
             &author_email=ada%40example.com&content=First+comment%21&website="
        ),
    );
    assert_eq!(resp.status(), 303, "form post should redirect");
    let location = resp.header("location").unwrap_or_default().to_owned();
    assert!(
        location.starts_with("/post/first-entry?comment="),
        "redirected to {location}"
    );

    // The site's default is `require_first`: a new commenter is held, and
    // told so rather than left wondering where their words went.
    assert!(
        location.contains("comment=pending"),
        "first-time comment should be held: {location}"
    );
    let held = get(base, "/post/first-entry");
    assert!(!held.contains("First comment!"), "held comment is public");
    let noticed = get(base, "/post/first-entry?comment=pending");
    assert!(
        noticed.contains("awaiting moderation"),
        "no moderation notice: {noticed}"
    );

    // Approving it publishes it to everyone — and the cached page is
    // dropped, so it appears at once rather than when the cache expires.
    approve_all(base);
    let after = get(base, "/post/first-entry");
    assert!(after.contains("First comment!"), "approved comment missing");
    assert!(after.contains("Ada Lovelace"), "commenter not shown");
    assert!(after.contains("1 comment"), "no count: {after}");
    assert!(after.contains("?reply_to="), "no reply link");
    assert!(
        !after.contains("awaiting moderation"),
        "a notice leaked into the cached page"
    );

    // Replying pre-fills a parent.
    let replying = get(base, "/post/first-entry?reply_to=1");
    assert!(
        replying.contains("name=\"parent_id\""),
        "reply form has no parent"
    );

    // The honeypot is accepted silently and stores nothing.
    let trapped = post_form(
        base,
        &format!(
            "post_id={post_id}&author_name=Bot&author_email=b%40e.com\
             &content=spam&website=http%3A%2F%2Fspam"
        ),
    );
    assert_eq!(trapped.status(), 303);
    let still = get(base, "/post/first-entry");
    assert!(!still.contains("spam"), "honeypot comment was stored");
    assert!(still.contains("1 comment"), "count changed");

    // An empty comment is refused, not stored.
    let empty = post_form(
        base,
        &format!("post_id={post_id}&author_name=&author_email=&content="),
    );
    assert_eq!(empty.status(), 303);
    assert!(
        empty
            .header("location")
            .unwrap_or_default()
            .contains("rejected"),
        "empty comment not reported as rejected"
    );
    1
}

/// Approves every pending comment through the admin API.
fn approve_all(base: &str) {
    let cookie = login(base);
    let listing = get_auth(base, &cookie, "/api/v1/comments?status=pending&limit=50");
    let rows: serde_json::Value = serde_json::from_str(&listing).expect("comments json");
    for row in rows.as_array().into_iter().flatten() {
        let id = row["id"].as_str().map_or_else(
            || {
                row["id"]
                    .as_i64()
                    .map(|n| n.to_string())
                    .unwrap_or_default()
            },
            str::to_owned,
        );
        let resp = http(
            ureq::post(format!("{base}/api/v1/comments/{id}/approve").as_str())
                .set("Cookie", &cookie)
                .send_string(""),
        );
        assert_eq!(resp.status(), 200, "approve {id}");
    }
}

/// The moderation setting an operator saves now reaches the service.
///
/// Every value the API accepted (`none`/`hold_new`/`all`) was a word the
/// service had never heard of, so it fell through to the default and the
/// setting did nothing whatever an operator chose.
fn moderation_is_honoured(base: &str, cookie: &str, _comment_id: i64) {
    let rejected = http(
        ureq::put(format!("{base}/api/v1/options").as_str())
            .set("Cookie", cookie)
            .send_json(serde_json::json!({"comment_moderation": "hold_new"})),
    );
    assert_eq!(
        rejected.status(),
        400,
        "a meaningless value must be refused"
    );

    let resp = http(
        ureq::put(format!("{base}/api/v1/options").as_str())
            .set("Cookie", cookie)
            .send_json(serde_json::json!({"comment_moderation": "auto_approve"})),
    );
    assert_eq!(resp.status(), 204, "set moderation");

    let posted = post_form(
        base,
        "post_id=86101&author_name=Straight+Through&author_email=s%40e.com\
         &content=Published+at+once&website=",
    );
    assert_eq!(posted.status(), 303);
    assert!(
        posted
            .header("location")
            .unwrap_or_default()
            .contains("published"),
        "auto_approve should publish immediately"
    );
    let page = get(base, "/post/second-entry");
    assert!(
        page.contains("Published at once"),
        "auto-approved comment missing: {page}"
    );
}

/// Listings, pagination, the search route and the feeds.
fn listings_and_feeds(base: &str) {
    // A derived excerpt appears even though neither post set one.
    let home = get(base, "/");
    assert!(
        home.contains("Body prose for the entry"),
        "no derived excerpt: {home}"
    );
    assert!(home.contains("Grace Hopper"), "no author on cards");

    // The search box points at the search route, and that route paginates.
    let search_page = get(base, "/search?s=entry");
    assert!(search_page.contains("First entry") || search_page.contains("Second entry"));
    // Page two of a two-result search with the default page size is empty
    // but must not be page one.
    let page_two = get(base, "/search?s=entry&page=2");
    assert_ne!(page_two, search_page, "?page= ignored by search");

    // The date archive the `archives` block links to exists.
    let now = chrono::Utc::now();
    let archive = http(
        ureq::get(
            format!(
                "{base}/archive/{}/{:02}",
                now.format("%Y"),
                now.format("%m")
            )
            .as_str(),
        )
        .call(),
    );
    assert_eq!(archive.status(), 200, "date archive route missing");
    assert!(body_text(archive).contains("First entry"));

    // The category archive lists its post.
    let category = get(base, "/category/rust");
    assert!(category.contains("First entry"), "category archive empty");

    // Feeds and the sitemap skip reusable patterns and link at real routes.
    let feed = get(base, "/feed.xml");
    assert!(feed.contains("/post/first-entry"), "feed item link");
    assert!(!feed.contains("a-pattern"), "block pattern in the feed");
    let sitemap = get(base, "/sitemap.xml");
    assert!(sitemap.contains("/post/first-entry"));
    assert!(
        !sitemap.contains("a-pattern"),
        "block pattern in the sitemap"
    );

    // An unknown URL gets the themed 404, not a bare string.
    let missing = http(ureq::get(format!("{base}/no-such-page").as_str()).call());
    assert_eq!(missing.status(), 404);
    let missing = body_text(missing);
    // Themed means the active theme's own tokens and chrome, not just the
    // built-in stylesheet that merely *references* the variables.
    assert!(
        missing.contains("--vy-color-primary: #123456"),
        "404 does not use the active theme's tokens"
    );
    assert!(missing.contains("vy-header"), "404 has no site chrome");
    assert!(missing.contains("noindex"), "404 should not be indexed");
}

/// What a crawler and a social scraper receive.
///
/// Every item here was missing or silently omitted: the canonical and
/// `og:url` because the site head had no fallback for an unconfigured
/// address (while the sitemap and feeds did), `og:image` because nothing
/// looked at the entry's own pictures, structured data because none was
/// emitted at all, and `Cache-Control` because no response carried one.
#[tokio::test]
async fn a_crawler_gets_canonical_structured_data_and_a_cache_policy() {
    let db = TestDb::new().await;
    let post_id = seed(db.pool()).await;
    // Deliberately unset: the address has to be derived from the request,
    // the way the sitemap and the feeds already derive it.
    sqlx::query("DELETE FROM options WHERE key = 'site_url'")
        .execute(db.pool())
        .await
        .expect("clear site_url");
    // An image in the body, so the social card has something to show.
    sqlx::query(
        "UPDATE posts SET content = '{\"schema_version\":1,\"blocks\":[
            {\"kind\":\"image\",\"attrs\":{\"mediaId\":4242,\"alt\":\"A picture\"}},
            {\"kind\":\"paragraph\",\"attrs\":{\"text\":\"Body prose for the entry.\"}}]}'
         WHERE id = $1",
    )
    .bind(post_id)
    .execute(db.pool())
    .await
    .expect("image block");

    let server = TestServer::start(common::BIN, &db);
    let base = server.base();
    let cookie = login(base);
    install_and_activate(base, &cookie);

    let entry = get(base, "/post/first-entry");

    // 1. Canonical and og:url, derived from the request the way the
    //    sitemap always has.
    assert!(
        entry.contains(&format!(
            "<link rel=\"canonical\" href=\"{base}/post/first-entry\">"
        )),
        "canonical missing:\n{entry}"
    );
    assert!(entry.contains("og:url"), "og:url missing");

    // 2. The entry's own picture becomes the social card.
    assert!(
        entry.contains("og:image") && entry.contains("/api/v1/media/4242/raw"),
        "og:image missing:\n{entry}"
    );
    assert!(
        entry.contains("summary_large_image"),
        "a card with an image is the large variant"
    );

    // 3. Structured data a search engine can read.
    let ld = entry
        .split("<script type=\"application/ld+json\">")
        .nth(1)
        .and_then(|s| s.split("</script>").next())
        .expect("a JSON-LD block");
    let parsed: serde_json::Value = serde_json::from_str(ld).expect("valid JSON");
    assert_eq!(parsed["@type"], "Article");
    assert_eq!(parsed["headline"], "First entry");
    assert!(parsed["datePublished"].is_string());
    assert_eq!(parsed["author"]["name"], "Grace Hopper");
    assert_eq!(
        parsed["mainEntityOfPage"]["@id"],
        format!("{base}/post/first-entry")
    );

    // The home page describes the site and its search endpoint instead.
    let home = get(base, "/");
    let ld = home
        .split("<script type=\"application/ld+json\">")
        .nth(1)
        .and_then(|s| s.split("</script>").next())
        .expect("a JSON-LD block on the index");
    let parsed: serde_json::Value = serde_json::from_str(ld).expect("valid JSON");
    assert_eq!(parsed["@type"], "WebSite");
    assert_eq!(parsed["potentialAction"]["@type"], "SearchAction");

    // 4. A cache policy, and an ETag that produces a 304.
    let response = http(ureq::get(format!("{base}/post/first-entry").as_str()).call());
    assert_eq!(
        response.header("cache-control"),
        Some("public, max-age=0, must-revalidate")
    );
    let etag = response.header("etag").expect("an ETag").to_owned();
    let conditional = http(
        ureq::get(format!("{base}/post/first-entry").as_str())
            .set("If-None-Match", &etag)
            .call(),
    );
    assert_eq!(conditional.status(), 304, "a repeat visit sends no body");
    assert_eq!(
        conditional.header("cache-control"),
        Some("public, max-age=0, must-revalidate"),
        "the 304 carries the policy too"
    );
}
