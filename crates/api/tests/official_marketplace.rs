//! The built-in marketplace and update channel, exercised against a
//! registry the test server hosts itself (debug-build test override).

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

mod common;

use serde_json::json;
use sha2::Digest as _;
use vyasa_db::models::Role;
use vyasa_testkit::{fixtures, TestDb, TestServer};

fn http(r: Result<ureq::Response, ureq::Error>) -> ureq::Response {
    match r {
        Ok(v) | Err(ureq::Error::Status(_, v)) => v,
        Err(ureq::Error::Transport(e)) => panic!("transport {e}"),
    }
}

fn body(r: ureq::Response) -> serde_json::Value {
    r.into_json().expect("json body")
}

fn sha(bytes: &[u8]) -> String {
    hex::encode(sha2::Sha256::digest(bytes))
}

/// A registry directory the server serves at `/registry/…`.
struct Registry {
    dir: tempfile::TempDir,
    secret: String,
    public: String,
}

impl Registry {
    fn new() -> Self {
        let (secret, public) = fixtures::keypair();
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("packages")).expect("packages dir");
        Self {
            dir,
            secret,
            public,
        }
    }

    fn package(&self, file: &str, bytes: &[u8]) {
        std::fs::write(self.dir.path().join("packages").join(file), bytes).expect("write package");
    }

    /// Writes the index naming `base` as the server's own address.
    fn index(&self, base: &str, listings: &[serde_json::Value]) {
        let text = serde_json::to_string(&json!({
            "schema": 1, "generated_at": "2026-10-04T00:00:00Z", "listings": listings
        }))
        .expect("index")
        .replace("http://BASE", base);
        std::fs::write(self.dir.path().join("index.json"), text).expect("write index");
    }

    fn version(file: &str, bytes: &[u8], signer: &str) -> serde_json::Value {
        json!({
            "version": "1.0.0", "url": format!("http://BASE/registry/packages/{file}"),
            "sha256": sha(bytes), "signature": fixtures::sign_hex(signer, bytes),
            "min_host_api": 0, "capabilities": ["log:write"], "released_at": "2026-10-04"
        })
    }

    fn server(&self, db: &TestDb) -> TestServer {
        TestServer::builder(common::BIN, db)
            .env("VYASA_REGISTRY_DIR", self.dir.path().display().to_string())
            .env("VYASA_TESTONLY_MARKETPLACE_URL", "/registry/index.json")
            .env("VYASA_TESTONLY_MARKETPLACE_KEYS", self.public.clone())
            .start()
    }
}

async fn admin_cookie(db: &TestDb, base: &str) -> String {
    let admin = common::seed_user(db.pool(), Role::Admin).await;
    common::login_cookie(base, &admin.email, &admin.password)
}

fn install(base: &str, cookie: &str, name: &str) -> ureq::Response {
    http(
        ureq::post(format!("{base}/api/v1/registry/install").as_str())
            .set("Cookie", cookie)
            .send_json(json!({
                "kind": "plugin", "name": name, "accept_capabilities": ["log:write"]
            })),
    )
}

#[tokio::test]
async fn browse_lists_the_official_index() {
    let db = TestDb::new().await;
    let reg = Registry::new();
    let server = reg.server(&db);
    let (author, author_pub) = fixtures::keypair();
    let pkg = fixtures::plugin_package("fx", "1.0.0", &["log:write"], &author);
    reg.package("fx-1.0.0.vyplugin", &pkg);
    reg.index(
        server.base(),
        &[
            json!({"kind": "plugin", "name": "fx", "title": "Fx", "author_key": author_pub,
            "versions": [Registry::version("fx-1.0.0.vyplugin", &pkg, &reg.secret)]}),
        ],
    );
    let cookie = admin_cookie(&db, server.base()).await;
    let resp = http(
        ureq::get(format!("{}/api/v1/registry?kind=plugin", server.base()).as_str())
            .set("Cookie", &cookie)
            .call(),
    );
    assert_eq!(resp.status(), 200);
    let doc = body(resp);
    assert_eq!(doc["state"], "official", "{doc}");
    assert_eq!(doc["entries"].as_array().map(Vec::len), Some(1), "{doc}");
}

#[tokio::test]
async fn a_package_signed_by_another_key_is_refused() {
    let db = TestDb::new().await;
    let reg = Registry::new();
    let server = reg.server(&db);
    let (author, author_pub) = fixtures::keypair();
    let (other, _) = fixtures::keypair();
    let pkg = fixtures::plugin_package("fx", "1.0.0", &["log:write"], &author);
    reg.package("fx-1.0.0.vyplugin", &pkg);
    reg.index(
        server.base(),
        &[
            json!({"kind": "plugin", "name": "fx", "author_key": author_pub,
            "versions": [Registry::version("fx-1.0.0.vyplugin", &pkg, &other)]}),
        ],
    );
    let cookie = admin_cookie(&db, server.base()).await;
    let resp = install(server.base(), &cookie, "fx");
    assert_eq!(resp.status(), 400);
    let msg = body(resp).to_string();
    assert!(msg.contains("does not match any trusted key"), "{msg}");
}

#[tokio::test]
async fn author_key_mismatch_is_refused() {
    let db = TestDb::new().await;
    let reg = Registry::new();
    let server = reg.server(&db);
    let (author_b, _) = fixtures::keypair();
    let (_, author_a_pub) = fixtures::keypair();
    let pkg = fixtures::plugin_package("fx", "1.0.0", &["log:write"], &author_b);
    reg.package("fx-1.0.0.vyplugin", &pkg);
    reg.index(
        server.base(),
        &[
            json!({"kind": "plugin", "name": "fx", "author_key": author_a_pub,
            "versions": [Registry::version("fx-1.0.0.vyplugin", &pkg, &reg.secret)]}),
        ],
    );
    let cookie = admin_cookie(&db, server.base()).await;
    let resp = install(server.base(), &cookie, "fx");
    assert_eq!(resp.status(), 400);
    let msg = body(resp).to_string();
    assert!(msg.contains("author"), "{msg}");
}

#[tokio::test]
async fn a_plugin_installs_with_only_the_listing_author_key() {
    let db = TestDb::new().await;
    let reg = Registry::new();
    let server = reg.server(&db);
    let (author, author_pub) = fixtures::keypair();
    let pkg = fixtures::plugin_package("fx", "1.0.0", &["log:write"], &author);
    reg.package("fx-1.0.0.vyplugin", &pkg);
    reg.index(
        server.base(),
        &[
            json!({"kind": "plugin", "name": "fx", "author_key": author_pub,
            "versions": [Registry::version("fx-1.0.0.vyplugin", &pkg, &reg.secret)]}),
        ],
    );
    let cookie = admin_cookie(&db, server.base()).await;
    let resp = install(server.base(), &cookie, "fx");
    assert!(
        resp.status() == 200 || resp.status() == 201,
        "install: {} {}",
        resp.status(),
        body(resp)
    );
    let doc = body(resp);
    assert_eq!(doc["needs_enabling"], true, "{doc}");
}

#[tokio::test]
async fn marketplace_off_refuses_browse_and_install() {
    let db = TestDb::new().await;
    let reg = Registry::new();
    let server = TestServer::builder(common::BIN, &db)
        .env("VYASA_REGISTRY_DIR", reg.dir.path().display().to_string())
        .env("VYASA_MARKETPLACE__ENABLED", "false")
        .start();
    let cookie = admin_cookie(&db, server.base()).await;
    let doc = body(http(
        ureq::get(format!("{}/api/v1/registry", server.base()).as_str())
            .set("Cookie", &cookie)
            .call(),
    ));
    assert_eq!(doc["configured"], false, "{doc}");
    assert_eq!(doc["state"], "off", "{doc}");
    let resp = install(server.base(), &cookie, "fx");
    assert_eq!(resp.status(), 400);
    assert!(body(resp).to_string().contains("turned off"));
}

// ---- the update channel ---------------------------------------------------

/// The registry host serves `<registry_dir>/index.json` byte for byte, so
/// a manifest placed there stands in for updates.vyasa.site.
fn manifest_registry(manifest: &serde_json::Value) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("index.json"), manifest.to_string()).expect("write manifest");
    dir
}

#[tokio::test]
async fn the_update_check_reads_the_official_channel() {
    let db = TestDb::new().await;
    let dir = manifest_registry(&json!({
        "latest": "9.9.9",
        "releases": [{"version": "9.9.9", "summary": "x", "artifacts": []}]
    }));
    let server = TestServer::builder(common::BIN, &db)
        .env("VYASA_REGISTRY_DIR", dir.path().display().to_string())
        .env("VYASA_TESTONLY_UPDATES_URL", "/registry/index.json")
        .start();
    let cookie = admin_cookie(&db, server.base()).await;
    let doc = body(http(
        ureq::get(format!("{}/api/v1/updates", server.base()).as_str())
            .set("Cookie", &cookie)
            .call(),
    ));
    assert_eq!(doc["update_available"], true, "{doc}");
    assert_eq!(doc["channel_state"], "official", "{doc}");
}

#[tokio::test]
async fn updates_off_is_reported_not_an_error() {
    let db = TestDb::new().await;
    let server = TestServer::builder(common::BIN, &db)
        .env("VYASA_UPDATES__ENABLED", "false")
        .start();
    let cookie = admin_cookie(&db, server.base()).await;
    let doc = body(http(
        ureq::get(format!("{}/api/v1/updates", server.base()).as_str())
            .set("Cookie", &cookie)
            .call(),
    ));
    assert_eq!(doc["channel_state"], "off", "{doc}");
    assert!(
        doc["channel_error"]
            .as_str()
            .is_some_and(|e| e.contains("update checks are turned off on this server")),
        "{doc}"
    );
}

// ---- hand-uploaded themes --------------------------------------------------

fn upload_theme(base: &str, cookie: &str, bytes: &[u8], signature: Option<&str>) -> ureq::Response {
    let boundary = "----vyasatheme";
    let mut body = Vec::new();
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; \
             filename=\"t.vytheme\"\r\nContent-Type: application/zip\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(bytes);
    if let Some(sig) = signature {
        body.extend_from_slice(
            format!(
                "\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"signature\"\r\n\r\n{sig}"
            )
            .as_bytes(),
        );
    }
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    http(
        ureq::post(format!("{base}/api/v1/themes").as_str())
            .set("Cookie", cookie)
            .set(
                "Content-Type",
                &format!("multipart/form-data; boundary={boundary}"),
            )
            .send_bytes(&body),
    )
}

#[tokio::test]
async fn scripted_theme_upload_needs_a_trusted_signature() {
    let db = TestDb::new().await;
    let (secret, public) = fixtures::keypair();
    let server = TestServer::builder(common::BIN, &db)
        .env("VYASA_PACKAGE_TRUSTED_KEYS", public)
        .start();
    let cookie = admin_cookie(&db, server.base()).await;
    let scripted = fixtures::theme_package("scripted", 1, &[("assets/theme.js", b"x()")]);

    let refused = upload_theme(server.base(), &cookie, &scripted, None);
    assert_eq!(refused.status(), 400);
    assert!(body(refused).to_string().contains("carries a script"));

    let sig = fixtures::sign_hex(&secret, &scripted);
    let accepted = upload_theme(server.base(), &cookie, &scripted, Some(&sig));
    assert_eq!(accepted.status(), 201, "{}", body(accepted));

    let plain = fixtures::data_theme_package("plain", 1);
    let fine = upload_theme(server.base(), &cookie, &plain, None);
    assert_eq!(fine.status(), 201, "{}", body(fine));
}
