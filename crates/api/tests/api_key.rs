//! End-to-end API-key auth tests against the real binary.
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod common;

use std::io::Read;

use serde_json::{json, Value};
use sqlx::PgPool;
use vyasa_db::models::Role;
use vyasa_db::repo::{ApiKeysRepo, NewUser, UsersRepo};
use vyasa_testkit::{TestDb, TestServer};

fn http(result: Result<ureq::Response, ureq::Error>) -> ureq::Response {
    match result {
        Ok(response) | Err(ureq::Error::Status(_, response)) => response,
        Err(ureq::Error::Transport(err)) => panic!("transport failure: {err}"),
    }
}

fn json_body(response: ureq::Response) -> Value {
    let mut body = String::new();
    response
        .into_reader()
        .take(1_000_000)
        .read_to_string(&mut body)
        .expect("read body");
    serde_json::from_str(&body).expect("parse json body")
}

/// Hashes a raw key the same way the api does (sha256 hex).
fn key_hash(raw: &str) -> String {
    use sha2::{Digest, Sha256};
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let digest = Sha256::digest(raw.as_bytes());
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push(HEX[usize::from(byte >> 4)] as char);
        out.push(HEX[usize::from(byte & 0x0f)] as char);
    }
    out
}

async fn seed(pool: &PgPool) -> (i64, i64, i64, i64) {
    let users = UsersRepo::new(pool.clone());
    let keys = ApiKeysRepo::new(pool.clone());
    let hash = vyasa_core::user::password::hash_password("pw-secret-1").expect("hash");

    // Admin with a fully-granted key and a limited key.
    #[allow(clippy::items_after_statements)]
    fn mk_user<'a>(
        id: i64,
        email: &'a str,
        username: &'a str,
        role: Role,
        hash: &'a str,
    ) -> NewUser<'a> {
        NewUser {
            id,
            email,
            username,
            display_name: username,
            password_hash: Some(hash),
            role,
            bio: "",
        }
    }
    users
        .insert(&mk_user(
            72_001,
            "headless@example.com",
            "headless",
            Role::Admin,
            &hash,
        ))
        .await
        .expect("insert admin");

    // Full key: grants manage_users.
    keys.insert(
        72_101,
        72_001,
        "full key",
        &key_hash("vy_full_key_0000000000000001"),
        &json!(["manage_users"]),
    )
    .await
    .expect("insert full key");

    // Limited key: no capabilities.
    let _limited = keys
        .insert(
            72_102,
            72_001,
            "limited key",
            &key_hash("vy_limited_key_000000000002"),
            &json!([]),
        )
        .await
        .expect("insert limited key");

    // Revoked key: granted, then revoked.
    let revoked = keys
        .insert(
            72_103,
            72_001,
            "revoked key",
            &key_hash("vy_revoked_key_00000000003"),
            &json!(["manage_users"]),
        )
        .await
        .expect("insert revoked key");
    keys.revoke(revoked.id).await.expect("revoke");

    (72_001, 72_101, 72_102, revoked.id)
}

#[tokio::test]
async fn api_key_auth_matrix() {
    let db = TestDb::new().await;
    let (_admin, _full, limited, _revoked) = seed(db.pool()).await;
    let server = TestServer::start(common::BIN, &db);
    let base = server.base();
    let _ = limited;

    let users_url = format!("{base}/api/v1/users").as_str().to_string();

    // 1. Valid key with the grant → 200.
    let ok = http(
        ureq::get(&users_url)
            .set("authorization", "Bearer vy_full_key_0000000000000001")
            .call(),
    );
    assert_eq!(ok.status(), 200, "granted key must list users");
    let list = json_body(ok);
    assert!(list.as_array().is_some_and(|a| !a.is_empty()));

    // 2. Key without the grant → 403 (cap not granted to the key).
    let forbidden = http(
        ureq::get(&users_url)
            .set("authorization", "Bearer vy_limited_key_000000000002")
            .call(),
    );
    assert_eq!(forbidden.status(), 403);
    assert_eq!(json_body(forbidden)["code"], "forbidden");

    // 3. Revoked key → 401.
    let revoked = http(
        ureq::get(&users_url)
            .set("authorization", "Bearer vy_revoked_key_00000000003")
            .call(),
    );
    assert_eq!(revoked.status(), 401);

    // 4. Unknown key → 401 with a distinct message.
    let unknown = http(
        ureq::get(&users_url)
            .set("authorization", "Bearer vy_does_not_exist")
            .call(),
    );
    assert_eq!(unknown.status(), 401);
    assert_eq!(json_body(unknown)["code"], "unauthorized");

    // 5. Malformed Authorization header (no Bearer prefix) → 401 via
    //    session path (no cookie).
    let no_bearer = http(
        ureq::get(&users_url)
            .set("authorization", "Basic dXNlcjpwYXNz")
            .call(),
    );
    assert_eq!(no_bearer.status(), 401);

    let full = || {
        http(
            ureq::get(&users_url)
                .set("authorization", "Bearer vy_full_key_0000000000000001")
                .call(),
        )
        .status()
    };

    // 6. A suspended owner's keys stop working; reinstated, they work
    //    again. Suspension used to end sessions only.
    sqlx::query("UPDATE users SET suspended_at = now() WHERE id = 72001")
        .execute(db.pool())
        .await
        .expect("suspend");
    assert_eq!(full(), 401, "suspended owner's key must not resolve");
    sqlx::query("UPDATE users SET suspended_at = NULL WHERE id = 72001")
        .execute(db.pool())
        .await
        .expect("reinstate");
    assert_eq!(full(), 200);

    // 7. A password reset revokes the account's keys along with its
    //    sessions: the old credential, however minted, no longer gets in.
    UsersRepo::new(db.pool().clone())
        .create_reset_token(72_001, &key_hash("vy-reset-token-for-key-test"))
        .await
        .expect("reset token");
    let reset = http(
        ureq::post(format!("{base}/api/v1/auth/reset").as_str()).send_json(json!({
            "token": "vy-reset-token-for-key-test",
            "password": "a-brand-new-password"
        })),
    );
    assert_eq!(reset.status(), 200);
    assert_eq!(full(), 401, "reset must revoke api keys");
}
