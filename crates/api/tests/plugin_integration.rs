//! What a plugin actually contributes to a running site.
//!
//! Boots a real server with the `bookshelf` example installed and checks
//! the parts that only exist once everything is wired together: a plugin
//! block rendered inside a post, a route served by guest code, a
//! stylesheet linked from every page, and a custom post type with public
//! URLs.
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod common;

use std::io::Write as _;

use sqlx::PgPool;
use vyasa_db::models::Role;
use vyasa_db::repo::NewUser;
use vyasa_testkit::{TestDb, TestServer};

const PLUGIN_WASM: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../plugin-sdk/examples/bookshelf/bookshelf-component.wasm"
);

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

/// A GET carrying the admin session cookie.
fn get_auth(base: &str, cookie: &str, path: &str) -> String {
    body_text(http(
        ureq::get(format!("{base}{path}").as_str())
            .set("Cookie", cookie)
            .call(),
    ))
}

fn get(base: &str, path: &str) -> String {
    body_text(http(ureq::get(format!("{base}{path}").as_str()).call())).replace("&#x2F;", "/")
}

/// A theme that renders post bodies and archive listings.
fn theme_package() -> Vec<u8> {
    let manifest = "name = \"plugintest\"\nversion = 1\nauthor = \"t\"\nrequired_api = 1\n";
    let tokens = r##"{"version":1,"colors":{"primary":{"light":"#123456"}}}"##;
    let layout = r#"{
      "index":[{"id":"posts","kind":"latest-posts"}],
      "single":[{"id":"body","kind":"post-content"}],
      "archive":[{"id":"list","kind":"latest-posts"}],
      "page":[{"id":"body","kind":"post-content"}],
      "search":[{"id":"find","kind":"search-box"}],
      "not-found":[{"id":"nf","kind":"content"}]}"#;
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

/// A page carrying a `bookshelf/rating` block, plus a `book` entry, plus
/// the plugin itself installed and enabled.
///
/// # Panics
/// If the wasm fixture at [`PLUGIN_WASM`] is missing: build it from
/// `plugin-sdk/examples/bookshelf` with `cargo build --release --target
/// wasm32-wasip2 && cp target/wasm32-wasip2/release/vyasa_plugin_bookshelf.wasm
/// bookshelf-component.wasm` (see that directory's README).
async fn seed(pool: &PgPool, decorate: bool) {
    let hash = vyasa_core::user::password::hash_password("pw-secret-1").unwrap();
    vyasa_db::repo::UsersRepo::new(pool.clone())
        .insert(&NewUser {
            id: 87_001,
            email: "plug-admin@example.com",
            username: "plugadmin",
            display_name: "Ada Lovelace",
            password_hash: Some(&hash),
            role: Role::Admin,
            bio: "",
        })
        .await
        .expect("admin");

    // A post containing the plugin's block. The document round-trips
    // through the same deserializer the editor writes through.
    sqlx::query(
        "INSERT INTO posts (id, author_id, type, status, slug, title, content, meta, published_at)
         VALUES (87100, 87001, 'post', 'published', 'a-review', 'A review',
                 '{\"schema_version\":1,\"blocks\":[
                    {\"kind\":\"paragraph\",\"attrs\":{\"text\":\"Prose around the block.\"}},
                    {\"kind\":\"bookshelf/rating\",\"attrs\":{\"stars\":4,\"title\":\"Dune\"}}]}',
                 '{}', now())",
    )
    .execute(pool)
    .await
    .expect("post with a plugin block");

    // An entry of the plugin's own post type.
    sqlx::query(
        "INSERT INTO posts (id, author_id, type, status, slug, title, content, meta, published_at)
         VALUES (87101, 87001, 'book', 'published', 'dune', 'Dune',
                 '{\"schema_version\":1,\"blocks\":[
                    {\"kind\":\"paragraph\",\"attrs\":{\"text\":\"A book entry.\"}}]}',
                 '{}', now())",
    )
    .execute(pool)
    .await
    .expect("book entry");

    seed_bound_page(pool).await;

    // A term in the taxonomy the plugin registers, on the book entry. The
    // rows exist before the plugin does: a taxonomy's terms outlive it,
    // which is exactly what `Taxonomy::from_db` is lax for.
    sqlx::query(
        "INSERT INTO terms (id, taxonomy, name, slug) VALUES (87300, 'shelf', 'Favourites', 'favourites')",
    )
    .execute(pool)
    .await
    .expect("term");
    sqlx::query("INSERT INTO term_relationships (post_id, term_id) VALUES (87101, 87300)")
        .execute(pool)
        .await
        .expect("term rel");

    let wasm = std::fs::read(PLUGIN_WASM).unwrap_or_else(|e| {
        panic!(
            "fixture missing at {PLUGIN_WASM} ({e}); from plugin-sdk/examples/bookshelf run \
             `cargo build --release --target wasm32-wasip2 && cp \
             target/wasm32-wasip2/release/vyasa_plugin_bookshelf.wasm bookshelf-component.wasm` \
             (see that directory's README)"
        )
    });
    let repo = vyasa_db::repo::PluginsRepo::new(pool.clone());
    let row = repo
        .create(
            "bookshelf",
            "0.1.0",
            &wasm,
            &vyasa_plugins::host::sha256_hex(&wasm),
            &serde_json::json!([
                "log:write",
                "db:read:posts",
                "db:read:options",
                "db:write:meta",
                "kv:read",
                "kv:write",
                "viewer:read",
                "mail:send"
            ]),
        )
        .await
        .expect("install plugin");
    repo.set_enabled(row.id, true).await.expect("enable");
    // The plugin decorates titles only when an operator asks it to.
    if decorate {
        repo.setting_put(row.id, "decorate", &serde_json::json!(true))
            .await
            .expect("setting");
    }
}

/// A page whose own section tree binds a `collection` to the plugin's
/// type. This is the phase-53 contract end to end: the plugin brings the
/// type, the page draws from it, and nothing names the plugin.
async fn seed_bound_page(pg: &sqlx::PgPool) {
    sqlx::query(
        "INSERT INTO posts (id, author_id, type, status, slug, title, content, meta, layout, published_at)
         VALUES (87102, 87001, 'page', 'published', 'shelf', 'Shelf',
                 '{\"schema_version\":1,\"blocks\":[]}', '{}',
                 '[{\"id\":\"grid\",\"kind\":\"collection\",
                    \"settings\":{\"bind\":{\"source\":\"book\",\"sort\":\"newest\",\"limit\":6},
                                  \"heading\":\"On the shelf\"}}]',
                 now())",
    )
    .execute(pg)
    .await
    .expect("bound page");

    // And a page composed with the plugin's own designer section.
    sqlx::query(
        "INSERT INTO posts (id, author_id, type, status, slug, title, content, meta, layout, published_at)
         VALUES (87103, 87001, 'page', 'published', 'library', 'Library',
                 '{\"schema_version\":1,\"blocks\":[]}', '{}',
                 '[{\"id\":\"shelf\",\"kind\":\"bookshelf/shelf-grid\",
                    \"settings\":{\"bind\":{\"source\":\"book\",\"sort\":\"newest\",\"limit\":6},
                                  \"heading\":\"Our books\"}}]',
                 now())",
    )
    .execute(pg)
    .await
    .expect("plugin-section page");
}

/// The plugin's own section: declared through `register-sections`,
/// rendered through `render-section`, sanitized and wrapped by the host,
/// with the binding resolved host-side — and offered to the studio and
/// the assistants through the vocabulary, tagged with its owner.
fn plugin_section_serves_the_page_and_the_vocabulary(base: &str, cookie: &str) {
    let library = get(base, "/library");
    assert!(library.contains("vy-plugin-section"), "wrapped:\n{library}");
    assert!(library.contains("Our books"), "heading:\n{library}");
    assert!(
        library.contains("bookshelf-spine"),
        "markup kept its class:\n{library}"
    );
    assert!(library.contains("/book/dune"), "bound entry:\n{library}");

    let vocab = get_auth(base, cookie, "/api/v1/themes/vocabulary");
    assert!(vocab.contains("bookshelf/shelf-grid"), "in vocabulary");
    assert!(
        vocab.contains("\"plugin\":\"bookshelf\""),
        "owner tagged:\n{}",
        &vocab[..vocab.len().min(400)]
    );
}

/// A page binding a `collection` to the plugin's type shows its entries
/// without one line of host code knowing what a book is.
fn bound_page_shows_the_books(base: &str) {
    let shelf_page = get(base, "/shelf");
    assert!(
        shelf_page.contains("On the shelf"),
        "bound page:\n{shelf_page}"
    );
    assert!(
        shelf_page.contains("vy-collection"),
        "bound page:\n{shelf_page}"
    );
    assert!(
        shelf_page.contains("Dune"),
        "bound page shows the book:\n{shelf_page}"
    );
    assert!(
        shelf_page.contains("/book/dune"),
        "cards link to the type's own URL space:\n{shelf_page}"
    );
}

fn login(base: &str) -> String {
    let resp = http(
        ureq::post(format!("{base}/api/v1/auth/login").as_str()).send_json(
            serde_json::json!({"email":"plug-admin@example.com","password":"pw-secret-1"}),
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

fn install_and_activate_theme(base: &str, cookie: &str) {
    let bytes = theme_package();
    let boundary = "----vyasaplug";
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
    assert_eq!(resp.status(), 201, "theme install");
    let themes: serde_json::Value = serde_json::from_str(&body_text(http(
        ureq::get(format!("{base}/api/v1/themes").as_str())
            .set("Cookie", cookie)
            .call(),
    )))
    .unwrap();
    let id = themes
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "plugintest")
        .map(|t| t["id"].as_i64().unwrap())
        .expect("installed");
    let resp = http(
        ureq::post(format!("{base}/api/v1/themes/{id}/activate").as_str())
            .set("Cookie", cookie)
            .send_string(""),
    );
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn a_plugin_reaches_the_page_the_route_and_the_url_space() {
    let db = TestDb::new().await;
    seed(db.pool(), true).await;
    let server = TestServer::start(common::BIN, &db);
    let base = server.base();
    let cookie = login(base);
    install_and_activate_theme(base, &cookie);

    // 1. The block. Its HTML is produced by guest code, sanitized by the
    //    host, and wrapped so a theme can style it.
    let page = get(base, "/post/a-review");
    assert!(
        page.contains("vy-plugin-block"),
        "no plugin block wrapper in:\n{page}"
    );
    assert!(page.contains("★★★★"), "four stars missing in:\n{page}");
    assert!(page.contains("Dune"), "block title missing");
    assert!(
        page.contains("Prose around the block."),
        "the rest of the post still renders"
    );

    // 2. The stylesheet the plugin enqueued, linked from the page and
    //    served at its content-addressed URL.
    let href = page
        .split("/plugin-assets/")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .expect("a plugin asset link in the head")
        .to_owned();
    assert!(
        std::path::Path::new(&href)
            .extension()
            .is_some_and(|e| e == "css"),
        "{href}"
    );
    let css = http(ureq::get(format!("{base}/plugin-assets/{href}").as_str()).call());
    assert_eq!(css.status(), 200);
    assert_eq!(css.header("content-type"), Some("text/css; charset=utf-8"));
    assert!(body_text(css).contains("bookshelf-rating"));

    // 3. The route, served by the plugin's own `handle-request`.
    let stats =
        http(ureq::get(format!("{base}/api/v1/plugin/bookshelf/stats?since=2020").as_str()).call());
    assert_eq!(stats.status(), 200);
    assert_eq!(stats.header("content-type"), Some("application/json"));
    assert_eq!(stats.header("x-content-type-options"), Some("nosniff"));
    // The plugin tries to set a cookie; the host drops every header that
    // is not on its allowlist.
    assert_eq!(
        stats.header("set-cookie"),
        None,
        "a plugin cannot set cookies"
    );
    let body = body_text(stats);
    assert!(body.contains("books"), "{body}");
    // The real query string reaches the guest — it used to be read from a
    // request header a caller could simply set.
    assert!(body.contains("since=2020"), "{body}");

    // An endpoint nobody declared is a 404, not a 200 echo.
    let missing = http(ureq::get(format!("{base}/api/v1/plugin/bookshelf/nope").as_str()).call());
    assert_eq!(missing.status(), 404);

    // 4. The custom post type: its own entry URL and its own archive.
    let entry = get(base, "/book/dune");
    assert!(entry.contains("A book entry."), "book entry:\n{entry}");
    let archive = get(base, "/book");
    assert!(archive.contains("Dune"), "book archive:\n{archive}");

    bound_page_shows_the_books(base);
    plugin_section_serves_the_page_and_the_vocabulary(base, &cookie);

    // 5. The declared task is recorded so an operator can see it, and the
    //    surface endpoint reports everything the plugin added.
    let surface: serde_json::Value = serde_json::from_str(&body_text(http(
        ureq::get(format!("{base}/api/v1/plugins/surface").as_str())
            .set("Cookie", &cookie)
            .call(),
    )))
    .unwrap();
    assert_eq!(surface["blocks"][0]["kind"], "bookshelf/rating");
    assert_eq!(surface["postTypes"][0]["slug"], "book");
    assert_eq!(surface["taxonomies"][0]["slug"], "shelf");
    assert_eq!(surface["tasks"][0]["name"], "tally");
    assert_eq!(surface["forms"][0]["fields"][0]["key"], "star");
    // The hook vocabulary itself, so the admin can show what is available.
    assert!(
        surface["filterPoints"]
            .as_array()
            .is_some_and(|a| a.iter().any(|p| p == "post-title")),
        "{surface}"
    );
    assert!(
        surface["eventNames"]
            .as_array()
            .is_some_and(|a| a.iter().any(|p| p == "post-saved")),
        "{surface}"
    );

    // 6. A custom taxonomy gets its own archive, sharing the `/{x}/{slug}`
    //    namespace with post types without either shadowing the other.
    let shelf = get(base, "/shelf/favourites");
    assert!(shelf.contains("Favourites"), "shelf archive:\n{shelf}");
    assert!(
        shelf.contains("A book entry.") || shelf.contains("Dune"),
        "the term's post is listed:\n{shelf}"
    );

    named_filter_points_run_on_real_pages(base);
}

/// The named filter points, asserted against pages a visitor receives.
fn named_filter_points_run_on_real_pages(base: &str) {
    // 7. The named filter points run on real pages.
    //
    //    `seo-title` has to reach the actual `<title>`, not only
    //    `og:title`: those are separate strings, and filtering one and not
    //    the other is how a hook ends up documented and reaching nothing.
    let home = get(base, "/");
    let title = home
        .split("<title>")
        .nth(1)
        .and_then(|t| t.split("</title>").next())
        .expect("a title tag");
    assert!(
        title.contains("· Bookshelf"),
        "seo-title in <title>: {title}"
    );
    assert!(
        home.contains("og:title") && home.contains("· Bookshelf"),
        "and in og:title too"
    );

    //    `post-title` runs wherever a title is shown to a visitor.
    assert!(home.contains("A review 📚"), "listing title:\n{home}");
    let entry = get(base, "/post/a-review");
    assert!(entry.contains("A review 📚"), "entry title:\n{entry}");

    //    `search-query` rewrites what is searched, not just what is shown.
    let search = get(base, "/search?s=novel");
    assert!(search.contains("book"), "rewritten query:\n{search}");
}

#[tokio::test]
async fn a_disabled_plugin_leaves_its_block_and_its_route_behind() {
    let db = TestDb::new().await;
    seed(db.pool(), false).await;
    // Disable it before boot: nothing it declared should be registered.
    sqlx::query("UPDATE plugins SET enabled = false")
        .execute(db.pool())
        .await
        .expect("disable");

    let server = TestServer::start(common::BIN, &db);
    let base = server.base();
    let cookie = login(base);
    install_and_activate_theme(base, &cookie);

    // The post still renders — the block becomes a comment a reader never
    // sees, and the prose around it is untouched. This is the case that
    // used to be impossible to reach at all, because a document holding a
    // plugin block could not be deserialized.
    let page = get(base, "/post/a-review");
    assert!(
        page.contains("Prose around the block."),
        "the post survives its plugin being disabled:\n{page}"
    );

    // The page bound to the plugin's type survives too: the collection
    // resolves to nothing rather than to cards linking into a URL space
    // that no longer answers. Re-enabling the plugin brings them back.
    let shelf_page = get(base, "/shelf");
    assert!(
        shelf_page.contains("Shelf"),
        "the bound page still renders:\n{shelf_page}"
    );
    assert!(
        !shelf_page.contains("/book/dune"),
        "no cards point into a dead URL space:\n{shelf_page}"
    );

    // The page composed with the plugin's own section survives too: the
    // section becomes a comment an author can find in the source, and
    // nothing a reader sees.
    let library = get(base, "/library");
    assert!(
        library.contains("Library"),
        "the composed page still renders:\n{library}"
    );
    assert!(
        library.contains("unresolved plugin section bookshelf/shelf-grid"),
        "the section degrades to a comment:\n{library}"
    );
    assert!(
        !library.contains("bookshelf-spine"),
        "no plugin markup without the plugin:\n{library}"
    );
    assert!(
        page.contains("unresolved plugin block bookshelf/rating"),
        "an unresolved block leaves a trace in the source:\n{page}"
    );
    assert!(!page.contains("★"), "nothing rendered for it");
    assert!(
        !page.contains("/plugin-assets/"),
        "a disabled plugin enqueues nothing"
    );

    let stats = http(ureq::get(format!("{base}/api/v1/plugin/bookshelf/stats").as_str()).call());
    assert_eq!(stats.status(), 404, "its route is gone too");
}

#[tokio::test]
async fn an_enabled_plugin_changes_nothing_it_was_not_asked_to() {
    let db = TestDb::new().await;
    // Enabled, with every capability it asked for, and no settings saved.
    seed(db.pool(), false).await;
    let server = TestServer::start(common::BIN, &db);
    let base = server.base();
    let cookie = login(base);
    install_and_activate_theme(base, &cookie);

    // Its own surface works…
    let stats = http(ureq::get(format!("{base}/api/v1/plugin/bookshelf/stats").as_str()).call());
    assert_eq!(stats.status(), 200);
    let entry = get(base, "/post/a-review");
    assert!(entry.contains("★★★★"), "its block still renders:\n{entry}");

    // …and the site's own content is untouched. Installing a plugin is not
    // consent for it to restyle every title on the site, and a filter that
    // decorates by default does exactly that to whoever enables it.
    let home = get(base, "/");
    assert!(!home.contains("📚"), "no badge on titles:\n{home}");
    assert!(!entry.contains("📚"), "nor on the entry:\n{entry}");
    let title = home
        .split("<title>")
        .nth(1)
        .and_then(|t| t.split("</title>").next())
        .expect("a title tag");
    assert!(
        !title.contains("Bookshelf"),
        "browser tab untouched: {title}"
    );
}

/// Enabling and disabling a plugin on a running server, without a restart.
#[tokio::test]
async fn a_plugin_can_be_turned_on_and_off_while_the_server_runs() {
    let db = TestDb::new().await;
    seed(db.pool(), false).await;
    let server = TestServer::start(common::BIN, &db);
    let base = server.base();
    let cookie = login(base);
    install_and_activate_theme(base, &cookie);

    let plugins: serde_json::Value =
        serde_json::from_str(&get_auth(base, &cookie, "/api/v1/plugins")).expect("json");
    let id = plugins
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "bookshelf")
        .map(|p| p["id"].as_i64().unwrap())
        .expect("installed");

    // Off: its route and its block go immediately.
    let off = http(
        ureq::post(format!("{base}/api/v1/plugins/{id}/disable").as_str())
            .set("Cookie", &cookie)
            .send_string(""),
    );
    assert_eq!(off.status(), 200);
    let stats = http(ureq::get(format!("{base}/api/v1/plugin/bookshelf/stats").as_str()).call());
    assert_eq!(stats.status(), 404, "its route stops answering");
    let entry = get(base, "/post/a-review");
    assert!(!entry.contains("★★★★"), "its block stops rendering");
    assert!(
        entry.contains("Prose around the block."),
        "the post survives"
    );

    // And a disabled plugin must not be *reported broken* for being
    // disabled. Leaving it registered on every hook meant each page render
    // called it, failed to load it, and marked it degraded — so turning a
    // plugin off made the admin say it had crashed.
    let listed: serde_json::Value =
        serde_json::from_str(&get_auth(base, &cookie, "/api/v1/plugins")).expect("json");
    let reported = listed
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "bookshelf")
        .map(|p| p["status"].as_str().unwrap_or_default().to_owned())
        .expect("listed");
    assert_ne!(reported, "degraded", "disabled is not broken");

    // On again: everything it declares comes back, with no restart.
    let on = http(
        ureq::post(format!("{base}/api/v1/plugins/{id}/enable").as_str())
            .set("Cookie", &cookie)
            .send_string(""),
    );
    assert_eq!(on.status(), 200);
    let stats = http(ureq::get(format!("{base}/api/v1/plugin/bookshelf/stats").as_str()).call());
    assert_eq!(stats.status(), 200, "its route answers again");
    let entry = get(base, "/post/a-review");
    assert!(entry.contains("★★★★"), "its block renders again:\n{entry}");
    let home = get(base, "/");
    assert!(
        home.contains("/plugin-assets/"),
        "its stylesheet is linked again"
    );
}
