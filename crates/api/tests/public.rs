//! Public-site integration: theme install → home → single → archive →
//! 404 → sitemap, plus preview-token and password flows.

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

fn body_text(r: ureq::Response) -> String {
    use std::io::Read;
    let mut s = String::new();
    r.into_reader()
        .take(2_000_000)
        .read_to_string(&mut s)
        .unwrap();
    s
}

/// Builds a minimal valid blog .vytheme in memory.
fn blog_package() -> Vec<u8> {
    blog_package_v(1, "#123456")
}

/// The same package at another version with another primary colour, so a
/// page can say which version rendered it.
fn blog_package_v(version: u32, primary: &str) -> Vec<u8> {
    let manifest =
        format!("name = \"pubtest\"\nversion = {version}\nauthor = \"t\"\nrequired_api = 1\n");
    let tokens = format!(r#"{{"version":1,"colors":{{"primary":{{"light":"{primary}"}}}}}}"#);
    let layout = r#"{"index":[{"id":"header","kind":"header"},{"id":"main","kind":"latest-posts"}],"single":[{"id":"header","kind":"header"},{"id":"body","kind":"post-content"}],"archive":[{"id":"main","kind":"latest-posts"}],"page":[{"id":"body","kind":"post-content"}],"search":[{"id":"main","kind":"search-box"}],"not-found":[{"id":"nf","kind":"content"}]}"#;
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut w = zip::ZipWriter::new(&mut buf);
        for (name, data) in [
            ("manifest.toml", manifest.as_bytes()),
            ("tokens.json", tokens.as_bytes()),
            ("layout.json", layout.as_bytes()),
        ] {
            w.start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap_or_else(|e| panic!("zip start {name}: {e}"));
            w.write_all(data).unwrap();
        }
        w.finish().unwrap();
    }
    buf.into_inner()
}

async fn seed_admin(pool: &PgPool) {
    let users = vyasa_db::repo::UsersRepo::new(pool.clone());
    let hash = vyasa_core::user::password::hash_password("pw-secret-1").unwrap();
    users
        .insert(&NewUser {
            id: 82_001,
            email: "admin-public@example.com",
            username: "adminpublic",
            display_name: "AdminPublic",
            password_hash: Some(&hash),
            role: Role::Admin,
            bio: "",
        })
        .await
        .expect("seed admin");
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn public_site_end_to_end() {
    let db = TestDb::new().await;
    seed_admin(db.pool()).await;
    let server = TestServer::start(common::BIN, &db);
    let base = server.base();
    let session = {
        let resp = http(
            ureq::post(format!("{base}/api/v1/auth/login").as_str())
                .send_json(json!({"email": "admin-public@example.com", "password": "pw-secret-1"})),
        );
        assert_eq!(resp.status(), 200);
        resp.all("set-cookie").join("; ")
    };

    // Install the blog package.
    let boundary = "XyZZyVyasa";
    let pkg = blog_package();
    let mut mp = Vec::new();
    mp.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"blog.vytheme\"\r\nContent-Type: application/zip\r\n\r\n").as_bytes());
    mp.extend_from_slice(&pkg);
    mp.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    let resp = http(
        ureq::post(format!("{base}/api/v1/themes").as_str())
            .set("Cookie", &session)
            .set(
                "Content-Type",
                &format!("multipart/form-data; boundary={boundary}"),
            )
            .send_bytes(&mp),
    );
    assert_eq!(resp.status(), 201, "{}", body_text(resp));
    let theme_id = {
        let list = http(
            ureq::get(format!("{base}/api/v1/themes").as_str())
                .set("Cookie", &session)
                .call(),
        );
        let v: serde_json::Value = serde_json::from_str(&body_text(list)).unwrap();
        v.as_array()
            .and_then(|rows| rows.iter().find(|r| r["name"] == "pubtest"))
            .and_then(|r| r["id"].as_i64())
            .expect("pubtest installed")
    };
    // Activate.
    let resp = http(
        ureq::post(format!("{base}/api/v1/themes/{theme_id}/activate").as_str())
            .set("Cookie", &session)
            .call(),
    );
    assert_eq!(resp.status(), 200);

    // Create + publish a post.
    let resp = http(
        ureq::post(format!("{base}/api/v1/posts").as_str())
            .set("Cookie", &session)
            .send_json(json!({
                "type": "post", "slug": "hello-world", "title": "Hello Public",
                "content": {"schema_version": 1, "blocks": [{"kind":"paragraph","attrs":{"text":"Visible body text."}}]},
                "excerpt": "short summary", "status": "draft"
            })),
    );
    assert_eq!(resp.status(), 201);
    let post: serde_json::Value = serde_json::from_str(&body_text(resp)).unwrap();
    let pid = post["id"].as_i64().unwrap();

    // Draft must NOT be public.
    let resp = http(ureq::get(format!("{base}/post/hello-world").as_str()).call());
    assert_eq!(resp.status(), 404);

    // Publish via API then it is visible on / and /post/{slug}.
    let resp = http(
        ureq::put(format!("{base}/api/v1/posts/{pid}").as_str())
            .set("Cookie", &session)
            .send_json(json!({"status": "published"})),
    );
    assert_eq!(resp.status(), 200);

    let resp = http(ureq::get(format!("{base}/").as_str()).call());
    assert_eq!(resp.status(), 200);
    let html = body_text(resp);
    assert!(html.contains("Hello Public"), "home shows post");

    let stored = http(
        ureq::get(format!("{base}/api/v1/posts/{pid}").as_str())
            .set("Cookie", &session)
            .call(),
    );
    eprintln!("STORED CONTENT: {}", body_text(stored));

    let resp = http(ureq::get(format!("{base}/post/hello-world").as_str()).call());
    assert_eq!(resp.status(), 200);
    let html = body_text(resp);
    assert!(html.contains("Visible body text"), "{html}");

    // 404 themed.
    let resp = http(ureq::get(format!("{base}/definitely-not-here").as_str()).call());
    assert_eq!(resp.status(), 404);

    // Sitemap lists the post permalink.
    let resp = http(ureq::get(format!("{base}/sitemap.xml").as_str()).call());
    assert_eq!(resp.status(), 200);
    assert!(body_text(resp).contains("/post/hello-world"));

    // Feeds parse and include the post title.
    let resp = http(ureq::get(format!("{base}/feed.xml").as_str()).call());
    assert_eq!(resp.status(), 200);
    assert!(body_text(resp).contains("Hello Public"));

    // Site identity: PUT site_title -> home reflects it after cache purge.
    let resp = http(
        ureq::put(format!("{base}/api/v1/options/site_title").as_str())
            .set("Cookie", &session)
            .send_json(json!("Renamed Site")),
    );
    assert_eq!(resp.status(), 204, "{}", body_text(resp));
    let resp = http(ureq::get(format!("{base}/").as_str()).call());
    let html = body_text(resp);
    assert!(
        html.contains("Renamed Site"),
        "site title reflected after purge"
    );

    // Custom CSS is served namespaced under #vy-site.
    let resp = http(
        ureq::put(format!("{base}/api/v1/options/custom_css").as_str())
            .set("Cookie", &session)
            .send_json(json!(
                "@import url(https://evil.example/x.css);\nh1{font-size:3rem}"
            )),
    );
    assert_eq!(resp.status(), 204);
    let resp = http(ureq::get(format!("{base}/custom.css").as_str()).call());
    assert_eq!(resp.status(), 200);
    let css = body_text(resp);
    assert!(css.contains("#vy-site"), "namespaced: {css}");
    assert!(css.contains("font-size:3rem"));
    assert!(!css.contains("@import"), "at-rule stripped");

    // Malicious custom CSS is emptied outright.
    let resp = http(
        ureq::put(format!("{base}/api/v1/options/custom_css").as_str())
            .set("Cookie", &session)
            .send_json(json!("p{width:expression(alert(1))}")),
    );
    assert_eq!(resp.status(), 204);
    let resp = http(ureq::get(format!("{base}/custom.css").as_str()).call());
    assert_eq!(resp.status(), 200, "sanitized-empty still served");
    let css = body_text(resp);
    assert!(!css.contains("expression"), "expression() stripped: {css}");

    // The bundle a request renders with is built once and kept, so a
    // theme change must reach it the moment it is activated: a visitor
    // must never see the old theme from a stale bundle.
    let home = body_text(http(ureq::get(format!("{base}/").as_str()).call()));
    assert!(home.contains("#123456"), "v1 colour on the page: {home}");
    let pkg = blog_package_v(2, "#654321");
    let mut mp = Vec::new();
    mp.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"blog.vytheme\"\r\nContent-Type: application/zip\r\n\r\n").as_bytes());
    mp.extend_from_slice(&pkg);
    mp.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    let resp = http(
        ureq::post(format!("{base}/api/v1/themes").as_str())
            .set("Cookie", &session)
            .set(
                "Content-Type",
                &format!("multipart/form-data; boundary={boundary}"),
            )
            .send_bytes(&mp),
    );
    assert_eq!(resp.status(), 201, "{}", body_text(resp));
    let v2_id = {
        let list = http(
            ureq::get(format!("{base}/api/v1/themes").as_str())
                .set("Cookie", &session)
                .call(),
        );
        let v: serde_json::Value = serde_json::from_str(&body_text(list)).unwrap();
        v.as_array()
            .and_then(|rows| {
                rows.iter()
                    .find(|r| r["name"] == "pubtest" && r["version"] == 2)
            })
            .and_then(|r| r["id"].as_i64())
            .expect("pubtest v2 installed")
    };
    let resp = http(
        ureq::post(format!("{base}/api/v1/themes/{v2_id}/activate").as_str())
            .set("Cookie", &session)
            .call(),
    );
    assert_eq!(resp.status(), 200);
    let home = body_text(http(ureq::get(format!("{base}/").as_str()).call()));
    assert!(
        home.contains("#654321"),
        "v2 colour after activation: {home}"
    );
    assert!(!home.contains("#123456"), "v1 bundle still served: {home}");
}

/// Deep links into the admin must be answered by the admin.
///
/// The public site registers `/{type}/{slug}`, which matches
/// `/admin/appearance` as readily as `/book/dune`. That route wins over the
/// fallback -- a fallback only runs when nothing matched -- so every
/// two-segment admin URL used to be served the public site's 404, rendered
/// in the site's theme. Reloading the admin turned it into the website.
///
/// Boots in a scratch directory holding a stub shell so the assertion is
/// about which handler answered, not about whether the UI happens to be
/// built next to the test: the admin index is read from a path relative
/// to the server's working directory, with no env override, so this test
/// points `TestServerBuilder::current_dir` at a directory of its own.
#[tokio::test]
async fn admin_deep_links_are_not_swallowed_by_the_public_site() {
    let db = TestDb::new().await;
    let dir = std::env::temp_dir().join(format!("vyasa-admin-route-{}", db.name()));
    std::fs::create_dir_all(dir.join("admin/dist")).expect("scratch dir");
    std::fs::write(
        dir.join("admin/dist/index.html"),
        "<!doctype html><title>Vyasa Admin</title><div id=root></div>",
    )
    .expect("stub shell");

    let server = TestServer::builder(common::BIN, &db)
        .current_dir(&dir)
        .start();
    let base = server.base();

    // Control: an unknown public URL is still the public site's problem.
    let miss = body_text(http(
        ureq::get(format!("{base}/no-such-page-here").as_str()).call(),
    ));
    assert!(
        !miss.contains("Vyasa Admin"),
        "public 404 should not be the admin shell: {miss}"
    );

    for path in [
        "/admin",
        // The SPA's index route puts this exact form in the address bar, and
        // `/admin/{*path}` does not match it: a catch-all wants a segment.
        "/admin/",
        "/admin/appearance",
        "/admin/plugins",
        "/admin/settings",
        "/admin/appearance/",
        "/admin/appearance?tab=design",
        "/admin/appearance/studio/some-draft-id",
    ] {
        let body = body_text(http(ureq::get(format!("{base}{path}").as_str()).call()));
        assert!(
            body.contains("Vyasa Admin"),
            "{path} was served by the public site, not the admin: {body}"
        );
    }

    // A 404 a browser is free to keep is how a fixed route stays broken.
    let miss = http(ureq::get(format!("{base}/still-no-such-page").as_str()).call());
    assert_eq!(miss.status(), 404);
    assert_eq!(
        miss.header("cache-control"),
        Some("no-store"),
        "a 404 must not be storable"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// Documents that something polls on a timer must be cacheable.
///
/// Feeds, the sitemap and robots.txt are fetched on a schedule by software
/// that never stops. Served with no `Cache-Control` and no validator, every
/// poll transferred the whole document and no shared cache would keep one.
#[tokio::test]
async fn polled_documents_carry_a_validator_and_a_lifetime() {
    let db = TestDb::new().await;
    let server = TestServer::start(common::BIN, &db);
    let base = server.base();

    for path in ["/feed.xml", "/atom.xml", "/sitemap.xml", "/robots.txt"] {
        let first = http(ureq::get(format!("{base}{path}").as_str()).call());
        assert_eq!(first.status(), 200, "{path}");

        let etag = first
            .header("etag")
            .unwrap_or_else(|| panic!("{path} has no ETag"))
            .to_owned();
        let cache = first
            .header("cache-control")
            .unwrap_or_else(|| panic!("{path} has no Cache-Control"))
            .to_owned();
        assert!(cache.contains("max-age="), "{path}: {cache}");
        let _ = body_text(first);

        // The whole point of the validator: asking again costs no body.
        let again = http(
            ureq::get(format!("{base}{path}").as_str())
                .set("If-None-Match", &etag)
                .call(),
        );
        assert_eq!(again.status(), 304, "{path} re-sent a body it need not");
        assert_eq!(
            again.header("etag"),
            Some(etag.as_str()),
            "{path}: a 304 without the validator leaves nothing to revalidate next time"
        );

        // A stale validator must still get the document.
        let changed = http(
            ureq::get(format!("{base}{path}").as_str())
                .set("If-None-Match", "\"not-the-current-one\"")
                .call(),
        );
        assert_eq!(changed.status(), 200, "{path} withheld a changed document");
    }
}

/// The sitemap has to name the page the site is actually at.
#[tokio::test]
async fn the_sitemap_lists_the_home_page_and_says_when_things_changed() {
    let db = TestDb::new().await;
    // A published post, so the `locs > 1` assertions below are not
    // vacuously true on an empty database (the home page alone is one
    // `<loc>`, and `if locs > 1` never ran on a fresh clone).
    seed_admin(db.pool()).await;
    sqlx::query(
        "INSERT INTO posts (id, author_id, type, status, slug, title, content, published_at)
         VALUES (82100, 82001, 'post', 'published', 'sitemap-probe', 'Sitemap probe',
                 '{\"schema_version\":1,\"blocks\":[{\"kind\":\"paragraph\",\"attrs\":{\"text\":\"Body.\"}}]}',
                 now())",
    )
    .execute(db.pool())
    .await
    .expect("post");
    let server = TestServer::start(common::BIN, &db);
    let base = server.base();
    let xml = body_text(http(
        ureq::get(format!("{base}/sitemap.xml").as_str()).call(),
    ));

    // It listed every post and page and omitted the one URL a crawler
    // looks for first.
    assert!(
        xml.contains("<loc>http://127.0.0.1") && xml.contains("/</loc>"),
        "no home page entry: {xml}"
    );

    // Every entry that names a URL says when it last changed, or a crawler
    // must choose between re-fetching everything and re-fetching nothing.
    let locs = xml.matches("<loc>").count();
    let mods = xml.matches("<lastmod>").count();
    assert!(locs > 1, "expected more than the home page alone: {xml}");
    assert_eq!(locs, mods, "{mods} lastmod for {locs} urls: {xml}");
    // Dates, not timestamps: to the second it changes on every
    // republish and stops carrying information.
    assert!(
        !xml.contains("<lastmod>") || xml.contains('-'),
        "lastmod is not a date: {xml}"
    );
}

/// A page number of zero is a request, not a crash.
///
/// `archive_impl` subtracted one from the raw page number. In debug and
/// test builds that is an overflow panic with no CatchPanic layer, so the
/// connection dropped with no response at all; in release it wrapped to
/// an OFFSET of four billion and an empty page. Unauthenticated, one URL.
#[tokio::test]
async fn page_zero_on_an_archive_is_answered_not_dropped() {
    let db = TestDb::new().await;
    let server = TestServer::start(common::BIN, &db);
    let base = server.base();
    for path in [
        "/category/anything?page=0",
        "/tag/anything?page=0",
        "/?page=0",
    ] {
        let resp = http(ureq::get(format!("{base}{path}").as_str()).call());
        assert!(
            resp.status() == 200 || resp.status() == 404,
            "{path}: {}",
            resp.status()
        );
    }
}
