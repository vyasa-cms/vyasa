//! The phase-55 proof: a store and a forum, built from two reference
//! plugins that use only the public contract — post types, meta, designer
//! sections, routes, the viewer boundary, mail — with the CMS supplying
//! everything else. If either of these needed a host-side special case,
//! phase 54's contract would be the bug.
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod common;

use sqlx::PgPool;
use vyasa_db::models::Role;
use vyasa_db::repo::NewUser;
use vyasa_testkit::{TestDb, TestServer};

const STOREFRONT_WASM: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../plugin-sdk/examples/storefront/storefront-component.wasm"
);
const FORUM_WASM: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../plugin-sdk/examples/forum-lite/forum-lite-component.wasm"
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

fn post_form(base: &str, cookie: &str, path: &str, form: &[(&str, &str)]) -> ureq::Response {
    http(
        ureq::post(format!("{base}{path}").as_str())
            .set("Cookie", cookie)
            .send_form(form),
    )
}

/// A tiny installable theme, enough for pages to render. The same shape
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
            "email": "shop-admin@example.com",
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

async fn seed(pool: &PgPool) {
    let hash = vyasa_core::user::password::hash_password("pw-secret-1").unwrap();
    vyasa_db::repo::UsersRepo::new(pool.clone())
        .insert(&NewUser {
            id: 88_001,
            email: "shop-admin@example.com",
            username: "shopadmin",
            display_name: "Shop Admin",
            password_hash: Some(&hash),
            role: Role::Admin,
            bio: "",
        })
        .await
        .expect("admin");

    // Two products: one published and priced through the plugin's route
    // later, one still a draft — which must never reach a bound section.
    sqlx::query(
        "INSERT INTO posts (id, author_id, type, status, slug, title, content, meta, published_at)
         VALUES (88101, 88001, 'product', 'published', 'clay-mug', 'Clay mug',
                 '{\"schema_version\":1,\"blocks\":[{\"kind\":\"paragraph\",\"attrs\":{\"text\":\"330ml stoneware.\"}}]}',
                 '{}', now()),
                (88102, 88001, 'product', 'draft', 'secret-vase', 'Secret vase',
                 '{\"schema_version\":1,\"blocks\":[]}', '{}', NULL)",
    )
    .execute(pool)
    .await
    .expect("products");

    // The store and forum pages: composed in the studio from the plugins'
    // sections — stored exactly as the studio would store them.
    sqlx::query(
        "INSERT INTO posts (id, author_id, type, status, slug, title, content, meta, layout, published_at)
         VALUES (88103, 88001, 'page', 'published', 'store', 'Store',
                 '{\"schema_version\":1,\"blocks\":[]}', '{}',
                 '[{\"id\":\"grid\",\"kind\":\"store/product-grid\",
                    \"settings\":{\"bind\":{\"source\":\"product\",\"sort\":\"newest\",\"limit\":8},
                                  \"heading\":\"New arrivals\"}}]',
                 now()),
                (88104, 88001, 'page', 'published', 'forum', 'Forum',
                 '{\"schema_version\":1,\"blocks\":[]}', '{}',
                 '[{\"id\":\"topics\",\"kind\":\"forum/topic-list\",
                    \"settings\":{\"bind\":{\"source\":\"topic\",\"sort\":\"newest\",\"limit\":10},
                                  \"heading\":\"Latest topics\"}}]',
                 now())",
    )
    .execute(pool)
    .await
    .expect("composed pages");

    install_plugin(
        pool,
        "storefront",
        STOREFRONT_WASM,
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
    install_plugin(
        pool,
        "forum-lite",
        FORUM_WASM,
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
}

#[tokio::test]
async fn a_store_and_a_forum_run_on_the_public_contract_alone() {
    let db = TestDb::new().await;
    seed(db.pool()).await;
    let server = TestServer::start(common::BIN, &db);
    let base = server.base();
    let cookie = login(base);
    install_and_activate_theme(base, &cookie);

    // --- the store ---

    // Pricing goes through the plugin's own route, admin-gated: price and
    // stock are meta the plugin owns, on an entry the CMS owns.
    let priced = post_form(
        base,
        &cookie,
        "/api/v1/plugin/storefront/price",
        &[("product", "88101"), ("value", "1450"), ("stock", "2")],
    );
    assert_eq!(priced.status(), 200, "{}", body_text(priced));

    // The composed store page shows the published product with its price —
    // and never the draft. Visibility rules live in the host's query path,
    // not in plugin code.
    let store = get(base, "/store");
    assert!(store.contains("New arrivals"), "store page:\n{store}");
    assert!(store.contains("Clay mug"), "store page:\n{store}");
    assert!(store.contains("1450"), "price shown:\n{store}");
    assert!(
        !store.contains("Secret vase"),
        "a draft product never renders in a bound section:\n{store}"
    );

    // The cart is signed-in only.
    let anon_cart = http(
        ureq::post(format!("{base}/api/v1/plugin/storefront/cart/add?product=88101").as_str())
            .send_string(""),
    );
    assert_eq!(anon_cart.status(), 401);
    let cart = post_form(
        base,
        &cookie,
        "/api/v1/plugin/storefront/cart/add",
        &[("product", "88101")],
    );
    let cart_body = body_text(cart);
    assert!(cart_body.contains("\"count\":1"), "{cart_body}");

    // Anyone may ask to order; the owner gets a mail, the visitor a page.
    let order = http(
        ureq::post(format!("{base}/api/v1/plugin/storefront/order").as_str()).send_form(&[
            ("product", "88101"),
            ("name", "Meera"),
            ("email", "meera@example.com"),
        ]),
    );
    let order_body = body_text(order);
    assert!(order_body.contains("Thank you"), "{order_body}");

    // --- the forum ---

    // A signed-in visitor starts a topic through the plugin route; the
    // plugin creates an ordinary entry and redirects to its public URL.
    // A no-redirect agent, so the 303 itself is visible instead of being
    // followed straight through to the topic page.
    let no_follow = ureq::builder().redirects(0).build();
    let created = http(
        no_follow
            .post(format!("{base}/api/v1/plugin/forum-lite/topics").as_str())
            .set("Cookie", &cookie)
            .send_form(&[
                ("title", "How do bindings work"),
                ("body", "Asking for a friend."),
            ]),
    );
    assert_eq!(created.status(), 303, "{}", body_text(created));
    let location = created.header("location").expect("redirect").to_owned();
    assert!(location.starts_with("/topic/"), "{location}");

    // The topic is a real entry at the type's own URL...
    let topic = get(base, &location);
    assert!(topic.contains("How do bindings work"), "topic:\n{topic}");
    assert!(topic.contains("Asking for a friend."), "topic:\n{topic}");

    // ...and the composed forum page lists it, with the reply count drawn
    // from the site's own comments.
    let forum = get(base, "/forum");
    assert!(forum.contains("Latest topics"), "forum:\n{forum}");
    assert!(forum.contains("How do bindings work"), "forum:\n{forum}");
    assert!(forum.contains("0 replies"), "forum:\n{forum}");
    assert!(forum.contains("Start a topic"), "forum:\n{forum}");

    // Writing needs an account; reading never did.
    let anon_topic = http(
        ureq::post(format!("{base}/api/v1/plugin/forum-lite/topics").as_str())
            .send_form(&[("title", "x"), ("body", "y")]),
    );
    assert_eq!(anon_topic.status(), 401);
}
