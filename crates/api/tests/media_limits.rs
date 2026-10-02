//! The media upload limit, end to end.
//!
//! `MAX_BYTES` says 10 MiB and the route documents a 413, but axum's
//! multipart extractor stops reading at its own 2 MiB default unless a
//! `DefaultBodyLimit` raises it, and the global body guard refused any
//! declared `Content-Length` over 2 MiB before the router saw it. Neither
//! was set, so every ordinary phone photo failed with a 400 and the 413
//! never fired. Only a real request through the real stack can prove both
//! layers are open.

#![allow(clippy::expect_used, clippy::unwrap_used)]

mod common;

use std::time::Duration;

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

/// A PNG by signature, padded to `len`. Sniffing goes by magic bytes;
/// nothing on the upload path decodes the image, so this is a real upload
/// as far as the limit is concerned.
fn png_of(len: usize) -> Vec<u8> {
    let mut bytes = vec![0u8; len];
    bytes[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
    bytes
}

fn multipart(file: &[u8]) -> (String, Vec<u8>) {
    let boundary = "----vyasamedia";
    let mut body = Vec::new();
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; \
             filename=\"photo.png\"\r\nContent-Type: image/png\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(file);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), body)
}

/// POSTs over a bare socket and returns the status code, surviving a
/// server that answers and hangs up before the body has been sent.
fn raw_post_status(base: &str, path: &str, cookie: &str, content_type: &str, body: &[u8]) -> u16 {
    use std::io::{Read as _, Write as _};
    let addr = base.trim_start_matches("http://");
    let mut sock = std::net::TcpStream::connect(addr).expect("connect");
    sock.set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let head = format!(
        "POST {path} HTTP/1.1\r\nHost: {addr}\r\nCookie: {cookie}\r\n\
         Content-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    sock.write_all(head.as_bytes()).expect("request head");
    // The body may never be wanted; a broken pipe here is the expected
    // shape of an early refusal, not a failure of the test.
    for chunk in body.chunks(64 * 1024) {
        if sock.write_all(chunk).is_err() {
            break;
        }
    }
    let _ = sock.shutdown(std::net::Shutdown::Write);
    let mut reply = Vec::new();
    let _ = sock.read_to_end(&mut reply);
    let text = String::from_utf8_lossy(&reply);
    let status_line = text.lines().next().unwrap_or_default();
    status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| panic!("no status line in reply: {text:.200}"))
}

async fn seed_admin(pool: &PgPool) {
    let hash = vyasa_core::user::password::hash_password("pw-secret-1").unwrap();
    vyasa_db::repo::UsersRepo::new(pool.clone())
        .insert(&NewUser {
            id: 91_001,
            email: "media-admin@example.com",
            username: "media-admin",
            display_name: "Media Admin",
            password_hash: Some(&hash),
            role: Role::Admin,
            bio: "",
        })
        .await
        .expect("admin");
}

fn login(base: &str) -> String {
    let login = http(
        ureq::post(format!("{base}/api/v1/auth/login").as_str()).send_json(
            serde_json::json!({"email": "media-admin@example.com", "password": "pw-secret-1"}),
        ),
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

#[tokio::test]
async fn uploads_between_two_and_ten_megabytes_succeed_and_larger_ones_get_a_413() {
    let db = TestDb::new().await;
    seed_admin(db.pool()).await;

    // Boots in a scratch directory, so the files the server writes land
    // nowhere near the source tree.
    let dir = std::env::temp_dir().join(format!("vyasa-media-limit-{}", db.name()));
    std::fs::create_dir_all(dir.join("media")).expect("scratch");
    let server = TestServer::builder(common::BIN, &db)
        .env("VYASA_MEDIA_DIR", dir.join("media").display().to_string())
        .start();
    let base = server.base();
    let cookie = login(base);

    let upload = |file: &[u8]| {
        let (ct, body) = multipart(file);
        http(
            ureq::post(format!("{base}/api/v1/media").as_str())
                .set("Cookie", &cookie)
                .set("Content-Type", &ct)
                .send_bytes(&body),
        )
    };

    // Three megabytes: an ordinary photo, and the case that used to fail.
    let ok = upload(&png_of(3 * 1024 * 1024));
    assert_eq!(ok.status(), 201, "{}", ok.into_string().unwrap_or_default());

    // Just over the cap but inside the envelope slack: the handler's own
    // check, which answers with the documented shape.
    let over = upload(&png_of(vyasa_core::media::MAX_BYTES + 16));
    assert_eq!(over.status(), 413);
    let body: serde_json::Value = over.into_json().expect("json error body");
    assert_eq!(body["code"], "payload_too_large", "{body}");
    assert!(
        body["message"].as_str().unwrap_or("").contains("10 MB"),
        "says what the limit is: {body}"
    );

    // Well over: the body guard refuses it from the declared length alone,
    // before a byte of the body is read, and closes. A client still
    // writing eleven megabytes sees a broken pipe before it sees the
    // status -- `ureq` reports that as a transport error -- so this one
    // goes over a raw socket that keeps writing through the close and then
    // reads whatever came back.
    let (ct, body) = multipart(&png_of(11 * 1024 * 1024));
    let status = raw_post_status(base, "/api/v1/media", &cookie, &ct, &body);
    assert_eq!(
        status, 413,
        "an oversize declared length must be refused early"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
