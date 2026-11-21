//! A theme that bundles a web app manifest has it asset and linked from the
//! public head; a theme without one has no link.

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

const LAYOUT: &str = r#"{"index":[{"id":"header","kind":"header"},{"id":"main","kind":"latest-posts"}],"single":[{"id":"header","kind":"header"},{"id":"body","kind":"post-content"}],"archive":[{"id":"main","kind":"latest-posts"}],"page":[{"id":"body","kind":"post-content"}],"search":[{"id":"main","kind":"search-box"}],"not-found":[{"id":"nf","kind":"content"}]}"#;

fn package(name: &str, extra: &[(&str, &[u8])]) -> Vec<u8> {
    let manifest = format!("name = \"{name}\"\nversion = 1\nauthor = \"t\"\nrequired_api = 1\n");
    let tokens = r##"{"version":1,"colors":{"primary":{"light":"#123456"}}}"##;
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut w = zip::ZipWriter::new(&mut buf);
        let mut files: Vec<(&str, &[u8])> = vec![
            ("manifest.toml", manifest.as_bytes()),
            ("tokens.json", tokens.as_bytes()),
            ("layout.json", LAYOUT.as_bytes()),
        ];
        files.extend_from_slice(extra);
        for (n, data) in files {
            w.start_file(n, zip::write::SimpleFileOptions::default())
                .unwrap();
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
            id: 82_101,
            email: "admin-manifest@example.com",
            username: "adminmanifest",
            display_name: "AdminManifest",
            password_hash: Some(&hash),
            role: Role::Admin,
            bio: "",
        })
        .await
        .expect("seed admin");
}

fn install_and_activate(base: &str, session: &str, name: &str, pkg: &[u8]) {
    let boundary = "XyZZyVyasaManifest";
    let mut mp = Vec::new();
    mp.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"t.vytheme\"\r\nContent-Type: application/zip\r\n\r\n").as_bytes());
    mp.extend_from_slice(pkg);
    mp.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    let resp = http(
        ureq::post(format!("{base}/api/v1/themes").as_str())
            .set("Cookie", session)
            .set(
                "Content-Type",
                &format!("multipart/form-data; boundary={boundary}"),
            )
            .send_bytes(&mp),
    );
    assert_eq!(resp.status(), 201, "{}", body_text(resp));
    let list = http(
        ureq::get(format!("{base}/api/v1/themes").as_str())
            .set("Cookie", session)
            .call(),
    );
    let v: serde_json::Value = serde_json::from_str(&body_text(list)).unwrap();
    let id = v
        .as_array()
        .and_then(|rows| rows.iter().find(|r| r["name"] == name))
        .and_then(|r| r["id"].as_i64())
        .expect("theme installed");
    let resp = http(
        ureq::post(format!("{base}/api/v1/themes/{id}/activate").as_str())
            .set("Cookie", session)
            .call(),
    );
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn a_bundled_manifest_is_served_and_linked_and_absent_otherwise() {
    let db = TestDb::new().await;
    seed_admin(db.pool()).await;
    let server = TestServer::start(common::BIN, &db);
    let base = server.base();
    let session = {
        let resp = http(
            ureq::post(format!("{base}/api/v1/auth/login").as_str()).send_json(
                json!({"email": "admin-manifest@example.com", "password": "pw-secret-1"}),
            ),
        );
        assert_eq!(resp.status(), 200);
        resp.all("set-cookie").join("; ")
    };

    // Without a manifest the head carries no link.
    install_and_activate(base, &session, "plainsite", &package("plainsite", &[]));
    let home = body_text(http(ureq::get(format!("{base}/").as_str()).call()));
    assert!(!home.contains("rel=\"manifest\""), "{home}");

    // With one the head links it, with a content hash, and the file is asset.
    let manifest = br##"{"name":"Site","theme_color":"#0b0b0b"}"##;
    install_and_activate(
        base,
        &session,
        "manifestsite",
        &package(
            "manifestsite",
            &[("assets/manifest/site.webmanifest", manifest)],
        ),
    );
    let home = body_text(http(ureq::get(format!("{base}/").as_str()).call()));
    assert!(
        home.contains("<link rel=\"manifest\" href=\"/theme-assets/manifest/site.webmanifest?v="),
        "{home}"
    );
    let asset =
        http(ureq::get(format!("{base}/theme-assets/manifest/site.webmanifest").as_str()).call());
    assert_eq!(asset.status(), 200);
    assert_eq!(
        asset.header("content-type"),
        Some("application/manifest+json")
    );
    assert!(body_text(asset).contains("theme_color"));
}
