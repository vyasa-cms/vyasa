//! The superset world, end to end against a real component.
//!
//! The fixture is the `bookshelf` example, built from
//! `plugin-sdk/examples/bookshelf` and committed the same way the `hello`
//! fixture is. It implements every v2 export, so these tests check the
//! host's side of each one against code that was actually compiled to
//! WebAssembly rather than against a mock.

#![allow(clippy::pedantic, clippy::unwrap_used, clippy::expect_used)]

use vyasa_plugins::broker::Broker;
use vyasa_plugins::host::{sha256_hex, Declaration, HostEnv, V2Outcome, WasmtimeHost};
use vyasa_testkit::TestDb;

const V2_FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../plugin-sdk/examples/bookshelf/bookshelf-component.wasm"
);

/// The base-world fixture, used to prove old plugins still load.
const V1_FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../plugin-sdk/examples/hello/hello-component.wasm"
);

/// The reference commerce plugin (phase 55).
const STOREFRONT_FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../plugin-sdk/examples/storefront/storefront-component.wasm"
);

/// The reference community plugin (phase 55).
const FORUM_FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../plugin-sdk/examples/forum-lite/forum-lite-component.wasm"
);

struct Fixture {
    _db: TestDb,
    pool: sqlx::PgPool,
    repo: vyasa_db::repo::PluginsRepo,
    host: WasmtimeHost,
    env: HostEnv,
    plugin_id: i64,
}

/// The rebuild command from the named example's own README (or, for
/// `hello`, `plugin-sdk/template`'s).
fn build_hint(name: &str) -> String {
    if name == "hello" {
        return "from plugin-sdk/template run `cargo build --target wasm32-wasip2 && cp \
                 target/wasm32-wasip2/debug/vyasa_plugin_template.wasm .` (see \
                 plugin-sdk/examples/hello/README.md)"
            .to_owned();
    }
    let crate_name = name.replace('-', "_");
    format!(
        "from plugin-sdk/examples/{name} run `cargo build --release --target wasm32-wasip2 && \
         cp target/wasm32-wasip2/release/vyasa_plugin_{crate_name}.wasm {name}-component.wasm` \
         (see that directory's README)"
    )
}

async fn setup(name: &str, wasm: &str, caps: serde_json::Value) -> Fixture {
    let bytes = std::fs::read(wasm).unwrap_or_else(|e| {
        panic!("fixture missing at {wasm} ({e}); {}", build_hint(name));
    });
    let db = TestDb::new().await;
    let pool = db.pool().clone();

    let repo = vyasa_db::repo::PluginsRepo::new(pool.clone());
    let sha = sha256_hex(&bytes);
    let row = repo
        .create(name, "0.1.0", &bytes, &sha, &caps)
        .await
        .expect("create");
    repo.set_enabled(row.id, true).await.expect("enable");

    let host = WasmtimeHost::new(repo.clone()).expect("engine");
    let env = HostEnv::background(
        std::sync::Arc::new(Broker::new(repo.clone(), pool.clone())),
        pool.clone(),
    );
    Fixture {
        _db: db,
        pool,
        repo,
        host,
        env,
        plugin_id: row.id,
    }
}

#[tokio::test]
async fn a_v2_plugin_answers_every_new_export() {
    let f = setup(
        "bookshelf",
        V2_FIXTURE,
        serde_json::json!(["log:write", "db:read:posts", "kv:read", "kv:write"]),
    )
    .await;

    assert!(
        f.host.supports_v2(f.plugin_id, &f.env).await.unwrap(),
        "the fixture implements the superset world"
    );

    // init persists the configured glyph through kv, which proves the
    // settings object reaches the guest and that kv survives the call.
    f.host
        .call_init(f.plugin_id, r#"{"star":"●"}"#, &f.env)
        .await
        .expect("no trap")
        .expect("accepted");
    let stored: String =
        sqlx::query_scalar("SELECT value FROM plugin_kv WHERE plugin_id = $1 AND key = $2")
            .bind(f.plugin_id)
            .bind("settings/star")
            .fetch_one(&f.pool)
            .await
            .expect("kv row");
    assert_eq!(stored, "●");

    // Declarations.
    for (which, needle) in [
        (Declaration::Schedule, "tally"),
        (Declaration::Admin, "star"),
        (Declaration::Assets, "bookshelf-rating"),
        (Declaration::PostTypes, "book"),
        (Declaration::Taxonomies, "shelf"),
    ] {
        match f.host.call_declaration(f.plugin_id, which, &f.env).await {
            V2Outcome::Ok(json) => assert!(json.contains(needle), "{which:?}: {json}"),
            other => panic!("{which:?}: {other:?}"),
        }
    }

    // render-block: the glyph comes from the setting stored at init, and
    // the title is escaped by the guest before the host sanitizes it.
    match f
        .host
        .call_render_block(
            f.plugin_id,
            "bookshelf/rating",
            r#"{"stars":3,"title":"<script>x</script>"}"#,
            &f.env,
        )
        .await
    {
        V2Outcome::Ok(html) => {
            assert!(html.contains("●●●"), "{html}");
            assert!(!html.contains("<script>"), "{html}");
        }
        other => panic!("render-block: {other:?}"),
    }

    // An unknown kind is the plugin's error, not a trap.
    match f
        .host
        .call_render_block(f.plugin_id, "other/thing", "{}", &f.env)
        .await
    {
        V2Outcome::Failed(reason) => assert!(reason.contains("other/thing"), "{reason}"),
        other => panic!("expected a refusal, got {other:?}"),
    }

    // handle-request.
    let request = vyasa_plugins::host::vyasa::plugin::host::HttpRequest {
        method: String::from("GET"),
        path: String::from("stats"),
        query: String::new(),
        headers: vec![],
        body: String::new(),
    };
    match f
        .host
        .call_handle_request(f.plugin_id, &request, &f.env)
        .await
    {
        V2Outcome::Ok(response) => {
            assert_eq!(response.status, 200);
            assert!(response.body.contains("books"), "{}", response.body);
        }
        other => panic!("handle-request: {other:?}"),
    }

    // run-task writes its result into kv.
    match f.host.call_run_task(f.plugin_id, "tally", &f.env).await {
        V2Outcome::Ok(()) => {}
        other => panic!("run-task: {other:?}"),
    }
    let count: Option<String> =
        sqlx::query_scalar("SELECT value FROM plugin_kv WHERE plugin_id = $1 AND key = $2")
            .bind(f.plugin_id)
            .bind("books/count")
            .fetch_optional(&f.pool)
            .await
            .unwrap();
    assert!(count.is_some(), "the task recorded a tally");

    // An unknown task is refused rather than silently succeeding.
    match f.host.call_run_task(f.plugin_id, "nope", &f.env).await {
        V2Outcome::Failed(reason) => assert!(reason.contains("nope"), "{reason}"),
        other => panic!("expected a refusal, got {other:?}"),
    }
    f.pool.close().await;
}

#[tokio::test]
async fn sections_register_and_render_through_the_string_keyed_points() {
    let f = setup(
        "bookshelf",
        V2_FIXTURE,
        serde_json::json!(["log:write", "kv:read", "kv:write"]),
    )
    .await;

    // Declaration: a JSON array of section descriptors, through the same
    // point mechanism every future surface uses — no new export, so every
    // installed binary that predates sections keeps loading.
    let decls = match f
        .host
        .call_filter_at(f.plugin_id, "register-sections", "", &f.env)
        .await
    {
        V2Outcome::Ok(reply) => {
            serde_json::from_str::<serde_json::Value>(&reply).expect("descriptors are JSON")
        }
        other => panic!("register-sections: {other:?}"),
    };
    let first = &decls.as_array().expect("array")[0];
    assert_eq!(first["kind"], "bookshelf/shelf-grid");
    assert_eq!(first["binds"], true);
    assert!(first["settings-schema"].is_object());

    // Render: the host resolves the binding and hands entries in; the
    // plugin turns them into markup and never touches the database.
    let payload = serde_json::json!({
        "kind": "bookshelf/shelf-grid",
        "settings": {"heading": "Our shelf"},
        "bound": [{"id": 1, "title": "Dune", "url": "/book/dune"}],
        "editor": false
    })
    .to_string();
    match f
        .host
        .call_filter_at(f.plugin_id, "render-section", &payload, &f.env)
        .await
    {
        V2Outcome::Ok(html) => {
            assert_ne!(html, payload, "the plugin claims its own kind");
            assert!(html.contains("Our shelf"), "{html}");
            assert!(html.contains("bookshelf-spine"), "{html}");
            assert!(html.contains("/book/dune"), "{html}");
        }
        other => panic!("render-section: {other:?}"),
    }

    // A kind it does not own comes back unchanged — the same "not mine"
    // contract every filter point keeps.
    let foreign =
        serde_json::json!({"kind": "acme/other", "settings": {}, "bound": []}).to_string();
    match f
        .host
        .call_filter_at(f.plugin_id, "render-section", &foreign, &f.env)
        .await
    {
        V2Outcome::Ok(out) => assert_eq!(out, foreign),
        other => panic!("render-section foreign: {other:?}"),
    }
}

#[tokio::test]
async fn the_storefront_declares_commerce_and_gates_its_cart() {
    let f = setup(
        "storefront",
        STOREFRONT_FIXTURE,
        serde_json::json!([
            "log:write",
            "db:read:posts",
            "db:write:meta",
            "kv:read",
            "kv:write",
            "viewer:read",
            "mail:send"
        ]),
    )
    .await;

    // Three commerce sections, all category "commerce", the grid binding
    // to the product type it registers itself.
    let decls = match f
        .host
        .call_filter_at(f.plugin_id, "register-sections", "", &f.env)
        .await
    {
        V2Outcome::Ok(reply) => serde_json::from_str::<serde_json::Value>(&reply).expect("JSON"),
        other => panic!("register-sections: {other:?}"),
    };
    let kinds: Vec<&str> = decls
        .as_array()
        .expect("array")
        .iter()
        .filter_map(|d| d["kind"].as_str())
        .collect();
    assert_eq!(
        kinds,
        [
            "store/product-grid",
            "store/product-hero",
            "store/buy-button"
        ]
    );

    // The grid renders bound entries; no price meta seeded means no price
    // shown, and no error either.
    let payload = serde_json::json!({
        "kind": "store/product-grid",
        "settings": {"heading": "New arrivals"},
        "bound": [{"id": 900_001, "title": "Clay mug", "url": "/product/clay-mug"}],
        "editor": false
    })
    .to_string();
    match f
        .host
        .call_filter_at(f.plugin_id, "render-section", &payload, &f.env)
        .await
    {
        V2Outcome::Ok(html) => {
            assert!(html.contains("store-card"), "{html}");
            assert!(html.contains("Clay mug"), "{html}");
        }
        other => panic!("render-section: {other:?}"),
    }

    // The cart is signed-in only: a request with no viewer is refused, and
    // the refusal explains itself instead of half-working.
    let refused = f
        .host
        .call_handle_request(
            f.plugin_id,
            &vyasa_plugins::host::vyasa::plugin::host::HttpRequest {
                method: "POST".to_owned(),
                path: "cart/add".to_owned(),
                query: "product=1".to_owned(),
                headers: vec![],
                body: String::new(),
            },
            &f.env,
        )
        .await;
    match refused {
        V2Outcome::Ok(res) => {
            assert_eq!(res.status, 401, "{}", res.body);
            assert!(res.body.contains("sign in"), "{}", res.body);
        }
        other => panic!("cart/add: {other:?}"),
    }
}

#[tokio::test]
async fn the_forum_lists_topics_and_gates_writing() {
    let f = setup(
        "forum-lite",
        FORUM_FIXTURE,
        serde_json::json!([
            "log:write",
            "db:read:posts",
            "db:read:comments",
            "db:write:posts",
            "db:publish:posts",
            "kv:read",
            "kv:write",
            "viewer:read"
        ]),
    )
    .await;

    let payload = serde_json::json!({
        "kind": "forum/topic-list",
        "settings": {"heading": "Latest topics"},
        "bound": [{"id": 900_002, "title": "How do bindings work?", "url": "/topic/how"}],
        "editor": false
    })
    .to_string();
    match f
        .host
        .call_filter_at(f.plugin_id, "render-section", &payload, &f.env)
        .await
    {
        V2Outcome::Ok(html) => {
            assert!(html.contains("forum-topic"), "{html}");
            assert!(html.contains("How do bindings work?"), "{html}");
            // Replies come from the site's own comments; a topic with none
            // says so rather than erroring on the empty count.
            assert!(html.contains("0 replies"), "{html}");
            assert!(html.contains("Start a topic"), "{html}");
        }
        other => panic!("render-section: {other:?}"),
    }

    // Reading needs no account; writing does.
    let refused = f
        .host
        .call_handle_request(
            f.plugin_id,
            &vyasa_plugins::host::vyasa::plugin::host::HttpRequest {
                method: "POST".to_owned(),
                path: "topics".to_owned(),
                query: String::new(),
                headers: vec![],
                body: "title=Hi&body=There".to_owned(),
            },
            &f.env,
        )
        .await;
    match refused {
        V2Outcome::Ok(res) => assert_eq!(res.status, 401, "{}", res.body),
        other => panic!("topics: {other:?}"),
    }
}

#[tokio::test]
async fn a_base_world_plugin_still_loads_and_reports_no_v2() {
    let f = setup("hello", V1_FIXTURE, serde_json::json!(["log:write"])).await;

    // The whole point of putting the new exports in a superset world: a
    // component built before any of them existed keeps working.
    assert!(
        !f.host.supports_v2(f.plugin_id, &f.env).await.unwrap(),
        "the base-world fixture must not claim v2"
    );
    f.host
        .call_init(f.plugin_id, "{}", &f.env)
        .await
        .expect("no trap")
        .expect("accepted");

    // And every v2 entry point answers "unsupported" rather than failing
    // the plugin, so nothing degrades it for not being from the future.
    assert!(matches!(
        f.host.call_run_task(f.plugin_id, "x", &f.env).await,
        V2Outcome::Unsupported
    ));
    assert!(matches!(
        f.host
            .call_render_block(f.plugin_id, "a/b", "{}", &f.env)
            .await,
        V2Outcome::Unsupported
    ));
    assert!(matches!(
        f.host
            .call_declaration(f.plugin_id, Declaration::Assets, &f.env)
            .await,
        V2Outcome::Unsupported
    ));
    assert_eq!(f.repo.list().await.unwrap()[0].status, "loaded");
    f.pool.close().await;
}

#[tokio::test]
async fn storage_is_refused_without_the_capability() {
    // Same component, no kv grants: init tries to store the glyph and the
    // host refuses, so the plugin rejects its own configuration.
    let f = setup(
        "bookshelf",
        V2_FIXTURE,
        serde_json::json!(["log:write", "db:read:posts"]),
    )
    .await;
    match f
        .host
        .call_init(f.plugin_id, r#"{"star":"●"}"#, &f.env)
        .await
    {
        Ok(Err(reason)) => assert_eq!(reason, "capability-denied"),
        other => panic!("expected a capability refusal, got {other:?}"),
    }
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM plugin_kv WHERE plugin_id = $1")
        .bind(f.plugin_id)
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(rows, 0, "nothing was written");
    f.pool.close().await;
}

#[tokio::test]
async fn named_hooks_carry_the_viewer_and_write_meta_and_mail() {
    let f = setup(
        "bookshelf",
        V2_FIXTURE,
        serde_json::json!([
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
    .await;
    let admin = seed_admin(&f.pool).await;
    let post_id = seed_post(&f.pool, admin).await;

    // The fixture decorates titles only when its `decorate` setting is on,
    // which it reads at `init` and stores. A demo that restyled every
    // title the moment it was enabled would do to a real site what this
    // one briefly did to ours.
    f.host
        .call_init(f.plugin_id, r#"{"decorate":true}"#, &f.env)
        .await
        .expect("no trap")
        .expect("accepted");

    // A filter point the plugin does not handle returns its payload
    // untouched — the property that lets every plugin see every point.
    match f
        .host
        .call_filter_at(f.plugin_id, "seo-description", "unchanged", &f.env)
        .await
    {
        V2Outcome::Ok(out) => assert_eq!(out, "unchanged"),
        other => panic!("filter-at: {other:?}"),
    }

    // A point it does handle transforms the payload.
    match f
        .host
        .call_filter_at(f.plugin_id, "post-title", "Dune", &f.env)
        .await
    {
        V2Outcome::Ok(out) => assert_eq!(out, "Dune 📚"),
        other => panic!("filter-at: {other:?}"),
    }

    // The viewer reaches a plugin that holds `viewer:read` — from a route,
    // which is rendered per request and cached by nobody.
    let as_viewer = HostEnv::background(std::sync::Arc::clone(&f.env.broker), f.env.dest.clone())
        .for_viewer(Some(vyasa_plugins::host::Viewer {
            id: admin,
            display_name: String::from("Ada"),
            role: String::from("admin"),
        }));
    let request = vyasa_plugins::host::vyasa::plugin::host::HttpRequest {
        method: String::from("GET"),
        path: String::from("stats"),
        query: String::new(),
        headers: vec![],
        body: String::new(),
    };
    match f
        .host
        .call_handle_request(f.plugin_id, &request, &as_viewer)
        .await
    {
        V2Outcome::Ok(response) => assert!(
            response.body.contains("\"viewer\":\"Ada\""),
            "{}",
            response.body
        ),
        other => panic!("handle-request: {other:?}"),
    }

    // And is absent everywhere else. This is the property that keeps a
    // cached page from being personalised for whoever happened to warm it.
    match f
        .host
        .call_handle_request(f.plugin_id, &request, &f.env)
        .await
    {
        V2Outcome::Ok(response) => assert!(
            response.body.contains("\"viewer\":\"anonymous\""),
            "{}",
            response.body
        ),
        other => panic!("handle-request: {other:?}"),
    }

    // A named event: the plugin stamps the post's own meta, namespaced
    // under its name so two plugins using `seen` never collide.
    let payload = format!(r#"{{"post_id":{post_id},"status":"draft","post_type":"post"}}"#);
    match f
        .host
        .call_on_event(f.plugin_id, "post-saved", &payload, &f.env)
        .await
    {
        V2Outcome::Ok(()) => {}
        other => panic!("on-event: {other:?}"),
    }
    let meta: serde_json::Value = sqlx::query_scalar("SELECT meta FROM posts WHERE id = $1")
        .bind(post_id)
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(meta["plugin"]["bookshelf"]["seen"], "yes", "{meta}");

    // Mail: queued as a job, and only to an address the site knows.
    sqlx::query(
        "INSERT INTO options (key, value) VALUES ('admin_email', '\"a@example.com\"')
         ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value",
    )
    .execute(&f.pool)
    .await
    .unwrap();
    match f
        .host
        .call_on_event(f.plugin_id, "user-created", r#"{"user_id":1}"#, &f.env)
        .await
    {
        V2Outcome::Ok(()) => {}
        other => panic!("on-event: {other:?}"),
    }
    let queued: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind = 'send_email'")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(queued, 1, "one message queued");
    f.pool.close().await;
}

#[tokio::test]
async fn the_viewer_and_meta_need_their_capabilities() {
    let f = setup(
        "bookshelf",
        V2_FIXTURE,
        // Everything except viewer:read and db:write:meta.
        serde_json::json!(["log:write", "db:read:posts", "kv:read", "kv:write"]),
    )
    .await;
    let admin = seed_admin(&f.pool).await;
    let post_id = seed_post(&f.pool, admin).await;

    // The plugin asks for the viewer, is refused, and falls back — a
    // denial is an error the guest handles, never a trap.
    let as_viewer = HostEnv::background(std::sync::Arc::clone(&f.env.broker), f.env.dest.clone())
        .for_viewer(Some(vyasa_plugins::host::Viewer {
            id: admin,
            display_name: String::from("Ada"),
            role: String::from("admin"),
        }));
    let request = vyasa_plugins::host::vyasa::plugin::host::HttpRequest {
        method: String::from("GET"),
        path: String::from("stats"),
        query: String::new(),
        headers: vec![],
        body: String::new(),
    };
    match f
        .host
        .call_handle_request(f.plugin_id, &request, &as_viewer)
        .await
    {
        V2Outcome::Ok(response) => assert!(
            response.body.contains("\"viewer\":\"anonymous\""),
            "no viewer:read, no viewer: {}",
            response.body
        ),
        other => panic!("handle-request: {other:?}"),
    }

    let payload = format!(r#"{{"post_id":{post_id}}}"#);
    match f
        .host
        .call_on_event(f.plugin_id, "post-saved", &payload, &f.env)
        .await
    {
        V2Outcome::Failed(reason) => assert_eq!(reason, "capability-denied"),
        other => panic!("expected a refusal, got {other:?}"),
    }
    let meta: serde_json::Value = sqlx::query_scalar("SELECT meta FROM posts WHERE id = $1")
        .bind(post_id)
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(meta, serde_json::json!({}), "nothing was written");
    f.pool.close().await;
}

async fn seed_admin(pool: &sqlx::PgPool) -> i64 {
    let id = vyasa_common::next_id_i64();
    sqlx::query(
        "INSERT INTO users (id, email, username, display_name, role)
         VALUES ($1, 'a@example.com', 'ada', 'Ada', 'admin')",
    )
    .bind(id)
    .execute(pool)
    .await
    .expect("admin");
    id
}

async fn seed_post(pool: &sqlx::PgPool, author: i64) -> i64 {
    let id = vyasa_common::next_id_i64();
    sqlx::query(
        "INSERT INTO posts (id, author_id, type, status, slug, title, content, meta)
         VALUES ($1, $2, 'post', 'draft', 'a-post', 'A post', '[]'::jsonb, '{}'::jsonb)",
    )
    .bind(id)
    .bind(author)
    .execute(pool)
    .await
    .expect("post");
    id
}

#[tokio::test]
async fn the_host_grants_exactly_what_the_manifest_asked_for() {
    // The capabilities a plugin *does* hold, and — just as importantly —
    // the ones it does not. Every refusal is an `Err` the guest can handle,
    // never a trap: a plugin reaching for something it did not declare
    // gets a message, and the request it was serving carries on.
    let f = setup(
        "bookshelf",
        V2_FIXTURE,
        serde_json::json!([
            "log:write",
            "db:read:posts",
            "db:read:options",
            "kv:read",
            "kv:write"
        ]),
    )
    .await;
    sqlx::query(
        "INSERT INTO options (key, value) VALUES ('site_title', '\"A site\"'),
                                                 ('admin_email', '\"a@example.com\"')
         ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value",
    )
    .execute(&f.pool)
    .await
    .unwrap();

    let request = vyasa_plugins::host::vyasa::plugin::host::HttpRequest {
        method: String::from("GET"),
        path: String::from("selftest"),
        query: String::new(),
        headers: vec![],
        body: String::new(),
    };
    let report: serde_json::Value = match f
        .host
        .call_handle_request(f.plugin_id, &request, &f.env)
        .await
    {
        V2Outcome::Ok(response) => serde_json::from_str(&response.body).expect("json report"),
        other => panic!("selftest: {other:?}"),
    };

    // Granted, and on the host's public-option list.
    assert_eq!(report["option_public"], "ok", "{report}");
    // Granted `db:read:options`, and still refused: `admin_email` is not
    // public, which is what makes the `site-admin` mail token necessary.
    assert_eq!(report["option_private"], "capability-denied", "{report}");
    assert_eq!(report["query_posts"], "ok:0", "{report}");
    assert_eq!(report["get_post"], "ok:none", "{report}");

    // Never asked for: each is refused by name.
    for key in [
        "emit_event",
        "fetch",
        "list_comments",
        "create_post",
        "ai_complete",
    ] {
        assert_eq!(report[key], "capability-denied", "{key}: {report}");
    }

    // And the denials are on the record.
    let denials: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM plugin_audit WHERE plugin_id = $1 AND kind = 'deny'",
    )
    .bind(f.plugin_id)
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert!(denials >= 5, "each refusal audited, got {denials}");
    f.pool.close().await;
}
