//! The docs plugin on a running server: pages, navigation, blocks, versions,
//! the Markdown mirror, `llms.txt`, scoped search and the webhook.
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::too_many_lines,
    clippy::too_many_arguments
)]

mod common;

use sqlx::PgPool;
use vyasa_db::models::Role;
use vyasa_db::repo::NewUser;
use vyasa_testkit::{TestDb, TestServer};

const DOCS_WASM: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../plugins/docs/docs-component.wasm"
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

fn get(base: &str, path: &str) -> String {
    body_text(http(ureq::get(format!("{base}{path}").as_str()).call())).replace("&#x2F;", "/")
}

/// every integration binary carries — a fixture, not a starter.
fn theme_package() -> Vec<u8> {
    use std::io::Write as _;
    let manifest = "name = \"reftest\"\nversion = 1\nauthor = \"t\"\nrequired_api = 1\n";
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

fn install_and_activate_theme(base: &str, cookie: &str) {
    let bytes = theme_package();
    let boundary = "----vyasaref";
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
        .find(|t| t["name"] == "reftest")
        .map(|t| t["id"].as_i64().unwrap())
        .expect("installed");
    let resp = http(
        ureq::post(format!("{base}/api/v1/themes/{id}/activate").as_str())
            .set("Cookie", cookie)
            .send_string(""),
    );
    assert_eq!(resp.status(), 200);
}

fn login(base: &str) -> String {
    let res = http(
        ureq::post(format!("{base}/api/v1/auth/login").as_str()).send_json(serde_json::json!({
            "email": "docs-admin@example.com",
            "password": "pw-secret-1"
        })),
    );
    let cookie = res
        .header("set-cookie")
        .expect("session cookie")
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    assert_eq!(res.status(), 200);
    cookie
}

/// # Panics
/// If the wasm fixture at `wasm_path` is missing: build it from
/// `plugin-sdk/examples/<name>` (see that directory's README).
async fn install_plugin(pool: &PgPool, name: &str, wasm_path: &str, caps: serde_json::Value) {
    let wasm = std::fs::read(wasm_path).unwrap_or_else(|e| {
        let crate_name = name.replace('-', "_");
        panic!(
            "fixture missing at {wasm_path} ({e}); from plugin-sdk/examples/{name} run `cargo \
             build --release --target wasm32-wasip2 && cp \
             target/wasm32-wasip2/release/vyasa_plugin_{crate_name}.wasm {name}-component.wasm` \
             (see that directory's README)"
        )
    });
    let repo = vyasa_db::repo::PluginsRepo::new(pool.clone());
    let row = repo
        .create(
            name,
            "0.1.0",
            &wasm,
            &vyasa_plugins::host::sha256_hex(&wasm),
            &caps,
        )
        .await
        .expect("install plugin");
    repo.set_enabled(row.id, true).await.expect("enable");
}

/// HMAC-SHA256 of `BODY` under the secret `shh`, as GitHub would send it.
const BODY: &str =
    r#"{"ref":"refs/heads/main","after":"0123456789abcdef0123456789abcdef01234567"}"#;
const SIGNATURE: &str = "sha256=eaf42c4fe87105793498154e1209853af3a98f03648005a8ea4855b3d13205cd";

fn doc_row(
    id: i64,
    parent: Option<i64>,
    slug: &str,
    path: &str,
    position: i32,
    title: &str,
    blocks: &str,
    fields: &str,
) -> String {
    format!(
        "({id}, 88001, 'doc', 'published', '{slug}', '{title}', '{{\"schema_version\":1,\"blocks\":{blocks}}}', \
         '{{\"fields\":{fields}}}', {parent}, '{path}', {position}, now())",
        parent = parent.map_or_else(|| "NULL".to_owned(), |p| p.to_string())
    )
}

async fn seed(pool: &PgPool) {
    let hash = vyasa_core::user::password::hash_password("pw-secret-1").unwrap();
    vyasa_db::repo::UsersRepo::new(pool.clone())
        .insert(&NewUser {
            id: 88_001,
            email: "docs-admin@example.com",
            username: "docsadmin",
            display_name: "Docs Admin",
            password_hash: Some(&hash),
            role: Role::Admin,
            bio: "",
        })
        .await
        .expect("admin");
    install_plugin(
        pool,
        "vyasa-docs",
        DOCS_WASM,
        serde_json::json!([
            "log:write",
            "kv:read",
            "kv:write",
            "entries:sync",
            "db:publish:posts",
            "net:fetch:api.github.com",
            "net:fetch:raw.githubusercontent.com",
            "tasks:extended",
            "assets:script"
        ]),
    )
    .await;
    let para =
        |t: &str| format!(r#"{{"kind":"paragraph","attrs":{{"text":"{t}"}},"children":[]}}"#);
    let h2 = |t: &str| {
        format!(r#"{{"kind":"heading","attrs":{{"level":2,"text":"{t}"}},"children":[]}}"#)
    };
    let tabs = format!(
        r#"{{"kind":"docs/tabs","attrs":{{"labels":["npm","pnpm"],"markdown":"**npm**\n\nnpm i"}},"children":[{{"kind":"group","attrs":{{}},"children":[{}]}},{{"kind":"group","attrs":{{}},"children":[{}]}}]}}"#,
        para("npm install zebra"),
        para("pnpm add zebra")
    );
    let rows = [
        // latest tree
        doc_row(
            88_101,
            None,
            "overview",
            "overview",
            10,
            "Overview",
            &format!("[{}]", para("Welcome zebra")),
            "{}",
        ),
        doc_row(
            88_102,
            None,
            "guides",
            "guides",
            20,
            "Guides",
            &format!("[{}]", para("All guides")),
            r#"{"section_label":"Learn"}"#,
        ),
        doc_row(
            88_103,
            Some(88_102),
            "install",
            "guides/install",
            10,
            "Install",
            &format!(
                "[{},{},{},{}]",
                h2("Requirements"),
                tabs,
                h2("Next"),
                para("Done")
            ),
            "{}",
        ),
        doc_row(
            88_104,
            Some(88_102),
            "secret",
            "guides/secret",
            20,
            "Secret",
            &format!("[{}]", para("Hidden from the sidebar")),
            r#"{"hide_from_nav":true}"#,
        ),
        doc_row(
            88_105,
            Some(88_102),
            "deploy",
            "guides/deploy",
            30,
            "Deploy",
            &format!("[{}]", para("Ship it")),
            "{}",
        ),
        // an older version
        doc_row(
            88_110,
            None,
            "v1",
            "v1",
            1010,
            "v1",
            &format!("[{}]", para("Docs for v1")),
            r#"{"version_root":true,"hide_from_nav":true}"#,
        ),
        doc_row(
            88_111,
            Some(88_110),
            "guides",
            "v1/guides",
            10,
            "Guides",
            &format!("[{}]", para("Old guides")),
            "{}",
        ),
        doc_row(
            88_112,
            Some(88_111),
            "install",
            "v1/guides/install",
            10,
            "Install",
            &format!("[{}]", para("Old install")),
            "{}",
        ),
    ];
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "INSERT INTO posts (id, author_id, type, status, slug, title, content, meta, parent_id, path, position, published_at) VALUES {}",
        rows.join(",")
    )))
    .execute(pool)
    .await
    .expect("docs");
    // Versions as terms, for scoped search.
    sqlx::query("INSERT INTO terms (id, taxonomy, name, slug) VALUES (88200, 'doc-version', 'v2', 'v2'), (88201, 'doc-version', 'v1', 'v1')")
        .execute(pool)
        .await
        .expect("terms");
    sqlx::query("INSERT INTO term_relationships (post_id, term_id) SELECT id, CASE WHEN path LIKE 'v1%' THEN 88201 ELSE 88200 END FROM posts WHERE type = 'doc'")
        .execute(pool)
        .await
        .expect("filed");
}

#[tokio::test]
async fn documentation_renders_navigates_mirrors_and_searches() {
    let db = TestDb::new().await;
    seed(db.pool()).await;
    let server = TestServer::start(common::BIN, &db);
    let base = server.base();
    let cookie = login(base);
    install_and_activate_theme(base, &cookie);

    // The page: sidebar (current page marked, hidden page absent, section label),
    // contents, pager, breadcrumbs, the tabs block with both panels.
    let page = get(base, "/docs/guides/install");
    assert!(
        page.contains("docs-nav") && page.contains("aria-current=\"page\""),
        "{page}"
    );
    assert!(page.contains("Learn"), "section label: {page}");
    assert!(
        page.contains("Deploy") && !page.contains(">Secret<"),
        "hidden pages are not in the sidebar: {page}"
    );
    assert!(
        page.contains("docs-toc") && page.contains("Requirements"),
        "{page}"
    );
    assert!(
        page.contains("docs-pager__prev") && page.contains("docs-pager__next"),
        "{page}"
    );
    assert!(page.contains("docs-crumbs"), "{page}");
    assert!(
        page.contains("role=\"tablist\"")
            && page.contains("npm install zebra")
            && page.contains("pnpm add zebra"),
        "{page}"
    );
    assert!(
        page.contains("docs-layout"),
        "the default layout's grid id is on the page: {page}"
    );
    assert!(
        !page.contains("class=\"docs-banner\""),
        "no version notice on the latest version: {page}"
    );
    // The plugin's stylesheet and script are linked from the head.
    assert!(
        page.contains("/plugin-assets/") && page.contains(".css"),
        "{page}"
    );

    // An older version: banner linking to the counterpart, canonical not set by the seed.
    let old = get(base, "/docs/v1/guides/install");
    assert!(
        old.contains("You are reading the documentation for")
            && old.contains("href=\"/docs/guides/install\""),
        "{old}"
    );
    assert!(
        old.contains("Old install") && !old.contains("Deploy"),
        "{old}"
    );

    // Addresses: the archive and search are redirects, the base replaces the slug.
    let agent = ureq::AgentBuilder::new().redirects(0).build();
    let loc = |path: &str| {
        let r = match agent.get(&format!("{base}{path}")).call() {
            Ok(r) | Err(ureq::Error::Status(_, r)) => r,
            Err(e) => panic!("{e}"),
        };
        (r.status(), r.header("location").map(str::to_owned))
    };
    assert_eq!(loc("/docs"), (302, Some("/docs/overview".to_owned())));
    assert_eq!(
        loc("/docs/search?q=zebra&term=doc-version:v1"),
        (
            302,
            Some("/search?s=zebra&type=doc&term=doc-version%3Av1".to_owned())
        )
    );
    assert_eq!(
        loc("/doc/guides/install"),
        (301, Some("/docs/guides/install".to_owned()))
    );

    // The Markdown mirror carries the plugin block's own Markdown; llms.txt and the sitemap list real paths.
    let md = get(base, "/docs/guides/install.md");
    assert!(
        md.starts_with("# Install") && md.contains("**npm**") && md.contains("npm i"),
        "{md}"
    );
    assert!(get(base, "/llms.txt").contains("/docs/guides/install.md"));
    assert!(get(base, "/sitemap.xml").contains("/docs/v1/guides/install"));

    // Scoped search: the index is built from the seeded rows.
    let r = http(
        ureq::post(&format!("{base}/api/v1/search/reindex"))
            .set("Cookie", &cookie)
            .send_string(""),
    );
    assert_eq!(r.status(), 200);
    let hits = |q: &str| -> Vec<String> {
        let v: serde_json::Value =
            serde_json::from_str(&get(base, &format!("/api/v1/search?q={q}"))).unwrap();
        let mut t: Vec<String> = v["hits"]
            .as_array()
            .unwrap()
            .iter()
            .map(|h| h["url"].as_str().unwrap().to_owned())
            .collect();
        t.sort();
        t
    };
    assert_eq!(
        hits("zebra&type=doc"),
        ["/docs/guides/install", "/docs/overview"]
    );
    assert_eq!(
        hits("install&type=doc&term=doc-version:v1"),
        ["/docs/v1/guides/install"]
    );
    assert!(hits("install&type=doc&term=doc-version:v3").is_empty());

    // The tree endpoint, public.
    let tree: serde_json::Value =
        serde_json::from_str(&get(base, "/api/v1/post-types/doc/tree")).unwrap();
    assert!(tree["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|n| n["url"] == "/docs/v1/guides/install"));
}

#[tokio::test]
async fn the_webhook_queues_a_sync_only_for_a_correctly_signed_push() {
    let db = TestDb::new().await;
    seed(db.pool()).await;
    let server = TestServer::start(common::BIN, &db);
    let base = server.base();
    let cookie = login(base);
    let plugin: i64 = sqlx::query_scalar("SELECT id FROM plugins WHERE name = 'vyasa-docs'")
        .fetch_one(db.pool())
        .await
        .unwrap();
    for (key, value) in [
        ("webhook_secret", "\"shh\""),
        ("repo_url", "\"https://github.com/o/r\""),
    ] {
        let r = http(
            ureq::put(&format!("{base}/api/v1/plugins/settings/{plugin}/{key}"))
                .set("Cookie", &cookie)
                .set("Content-Type", "application/json")
                .send_string(value),
        );
        assert_eq!(r.status(), 204, "{key}");
    }
    let post = |signature: Option<&str>, event: &str| {
        let mut req = ureq::post(&format!("{base}/api/v1/plugin/vyasa-docs/webhook"))
            .set("Content-Type", "application/json")
            .set("X-GitHub-Event", event);
        if let Some(sig) = signature {
            req = req.set("X-Hub-Signature-256", sig);
        }
        http(req.send_string(BODY)).status()
    };
    let queued = || async {
        sqlx::query_scalar::<_, String>(
            "SELECT value FROM plugin_kv WHERE plugin_id = $1 AND key = 'sync/pending'",
        )
        .bind(plugin)
        .fetch_optional(db.pool())
        .await
        .unwrap()
    };
    assert_eq!(post(None, "push"), 401);
    assert_eq!(post(Some("sha256=00"), "push"), 401);
    assert_eq!(queued().await, None, "a bad signature queues nothing");
    assert_eq!(post(Some(SIGNATURE), "ping"), 200);
    assert_eq!(queued().await, None, "a ping queues nothing");
    assert_eq!(post(Some(SIGNATURE), "push"), 202);
    assert_eq!(
        queued().await.as_deref(),
        Some("0123456789abcdef0123456789abcdef01234567")
    );
}
