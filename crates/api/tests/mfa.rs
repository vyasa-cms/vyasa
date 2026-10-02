//! A second factor over HTTP: setup, confirm, the 202 challenge at
//! sign-in, a code that finishes it, a recovery code that also does, and
//! the administrator's reset.
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod common;

use serde_json::{json, Value};
use vyasa_testkit::{TestDb, TestServer};

const TOKEN: &str = "stp_test_token_for_mfa_000001";
const PW: &str = "twelve-char-password";
const SECRET_KEY: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn http(r: Result<ureq::Response, ureq::Error>) -> ureq::Response {
    match r {
        Ok(v) | Err(ureq::Error::Status(_, v)) => v,
        Err(ureq::Error::Transport(e)) => panic!("transport {e}"),
    }
}

fn json_body(r: ureq::Response) -> Value {
    use std::io::Read;
    let mut s = String::new();
    r.into_reader()
        .take(1_000_000)
        .read_to_string(&mut s)
        .unwrap();
    serde_json::from_str(&s).unwrap_or(Value::Null)
}

fn cookie_of(r: &ureq::Response, name: &str) -> Option<String> {
    r.all("set-cookie")
        .iter()
        .filter_map(|c| c.split(';').next())
        .find(|kv| kv.starts_with(name))
        .map(std::string::ToString::to_string)
}

fn code_for(secret_b32: &str) -> String {
    let secret = totp_rs::Secret::try_from_base32(secret_b32).unwrap();
    totp_rs::Builder::new()
        .with_algorithm(totp_rs::Algorithm::SHA1)
        .with_digits(6)
        .with_skew(1)
        .with_step_duration(30)
        .with_secret(secret)
        .with_account_name("x@example.com")
        .with_issuer(Some("t"))
        .build()
        .unwrap()
        .generate_current()
        .to_string()
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn a_second_factor_gates_sign_in_until_a_code_or_recovery_code_is_given() {
    let db = TestDb::new().await;
    let server = TestServer::builder(common::BIN, &db)
        .env("VYASA_SETUP_TOKEN", TOKEN)
        .env("VYASA_SECRET_KEY", SECRET_KEY)
        .start();
    let base = server.base();
    let claimed = http(
        ureq::post(format!("{base}/api/v1/setup/claim").as_str())
            .send_json(json!({"token": TOKEN})),
    );
    let setup_cookie = cookie_of(&claimed, "vy_setup").unwrap();
    let created = http(
        ureq::post(format!("{base}/api/v1/setup/account").as_str())
            .set("cookie", &setup_cookie)
            .send_json(json!({"email": "x@example.com", "password": PW})),
    );
    let session = cookie_of(&created, "vy_session").unwrap();

    // Off to begin with; setup hands back a secret and a QR; a wrong code
    // does not turn it on.
    let status = json_body(http(
        ureq::get(format!("{base}/api/v1/auth/mfa/status").as_str())
            .set("cookie", &session)
            .call(),
    ));
    assert_eq!(status["enabled"], false);
    let setup = json_body(http(
        ureq::post(format!("{base}/api/v1/auth/mfa/setup").as_str())
            .set("cookie", &session)
            .call(),
    ));
    let secret = setup["secret"].as_str().unwrap().to_owned();
    assert!(setup["otpauth_url"]
        .as_str()
        .unwrap()
        .starts_with("otpauth://totp/"));
    assert!(setup["qr_svg"].as_str().unwrap().contains("<svg"));
    let wrong = http(
        ureq::post(format!("{base}/api/v1/auth/mfa/confirm").as_str())
            .set("cookie", &session)
            .send_json(json!({"code": "000000"})),
    );
    assert_eq!(wrong.status(), 400);
    let confirmed = json_body(http(
        ureq::post(format!("{base}/api/v1/auth/mfa/confirm").as_str())
            .set("cookie", &session)
            .send_json(json!({"code": code_for(&secret)})),
    ));
    let codes: Vec<String> = confirmed["codes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c.as_str().unwrap().to_owned())
        .collect();
    assert_eq!(codes.len(), 8);

    // An existing session cannot silently replace an enabled factor.
    let restarted = http(
        ureq::post(format!("{base}/api/v1/auth/mfa/setup").as_str())
            .set("cookie", &session)
            .call(),
    );
    assert_eq!(restarted.status(), 400);
    let still_enabled = json_body(http(
        ureq::get(format!("{base}/api/v1/auth/mfa/status").as_str())
            .set("cookie", &session)
            .call(),
    ));
    assert_eq!(still_enabled["enabled"], true);
    assert_eq!(still_enabled["recovery_codes_left"], 8);

    // Password lookup/session creation still work when only the MFA query
    // fails. That failure must never yield a signed-in cookie.
    sqlx::query("ALTER TABLE user_mfa RENAME TO user_mfa_unavailable")
        .execute(db.pool())
        .await
        .unwrap();
    let failed_lookup = http(
        ureq::post(format!("{base}/api/v1/auth/login").as_str())
            .send_json(json!({"email": "x@example.com", "password": PW})),
    );
    sqlx::query("ALTER TABLE user_mfa_unavailable RENAME TO user_mfa")
        .execute(db.pool())
        .await
        .unwrap();
    assert_eq!(failed_lookup.status(), 500);
    assert!(cookie_of(&failed_lookup, "vy_session").is_none());

    // Password alone now answers 202 with a challenge and no cookie.
    let first = http(
        ureq::post(format!("{base}/api/v1/auth/login").as_str())
            .send_json(json!({"email": "x@example.com", "password": PW})),
    );
    assert_eq!(first.status(), 202);
    assert!(
        cookie_of(&first, "vy_session").is_none(),
        "no session before the code"
    );
    let challenge = json_body(first)["challenge"].as_str().unwrap().to_owned();
    let bad = http(
        ureq::post(format!("{base}/api/v1/auth/mfa").as_str())
            .send_json(json!({"challenge": challenge, "code": "123456"})),
    );
    assert_eq!(bad.status(), 401);
    let good = http(
        ureq::post(format!("{base}/api/v1/auth/mfa").as_str())
            .send_json(json!({"challenge": challenge, "code": code_for(&secret)})),
    );
    assert_eq!(good.status(), 200);
    assert!(cookie_of(&good, "vy_session").is_some());

    // A recovery code works once.
    let again = json_body(http(
        ureq::post(format!("{base}/api/v1/auth/login").as_str())
            .send_json(json!({"email": "x@example.com", "password": PW})),
    ));
    let challenge2 = again["challenge"].as_str().unwrap().to_owned();
    let rec = http(
        ureq::post(format!("{base}/api/v1/auth/mfa").as_str())
            .send_json(json!({"challenge": challenge2, "code": codes[0]})),
    );
    assert_eq!(rec.status(), 200);
    let session2 = cookie_of(&rec, "vy_session").unwrap();
    let status = json_body(http(
        ureq::get(format!("{base}/api/v1/auth/mfa/status").as_str())
            .set("cookie", &session2)
            .call(),
    ));
    assert_eq!(status["recovery_codes_left"], 7);
    let third = json_body(http(
        ureq::post(format!("{base}/api/v1/auth/login").as_str())
            .send_json(json!({"email": "x@example.com", "password": PW})),
    ));
    let spent = http(
        ureq::post(format!("{base}/api/v1/auth/mfa").as_str())
            .send_json(json!({"challenge": third["challenge"], "code": codes[0]})),
    );
    assert_eq!(spent.status(), 401, "a recovery code is spent on use");

    // The users list says who has it on; an administrator can reset it.
    let me = json_body(http(
        ureq::get(format!("{base}/api/v1/auth/me").as_str())
            .set("cookie", &session2)
            .call(),
    ));
    let users = json_body(http(
        ureq::get(format!("{base}/api/v1/users").as_str())
            .set("cookie", &session2)
            .call(),
    ));
    assert_eq!(users[0]["mfa_enabled"], true);
    let reset = http(
        ureq::delete(format!("{base}/api/v1/users/{}/mfa", me["id"]).as_str())
            .set("cookie", &session2)
            .call(),
    );
    assert_eq!(reset.status(), 204);
    let plain = http(
        ureq::post(format!("{base}/api/v1/auth/login").as_str())
            .send_json(json!({"email": "x@example.com", "password": PW})),
    );
    assert_eq!(plain.status(), 200, "password alone signs in again");

    // Without VYASA_SECRET_KEY the challenge key used to be derived from
    // the constant `vyasa-insecure`, so anyone could sign a challenge for
    // any user id and skip the password. Turn the factor on again under a
    // server with no key and try exactly that.
    let server2 = TestServer::builder(common::BIN, &db)
        .env("VYASA_SETUP_TOKEN", TOKEN)
        .start();
    let base2 = server2.base();
    let signed_in = http(
        ureq::post(format!("{base2}/api/v1/auth/login").as_str())
            .send_json(json!({"email": "x@example.com", "password": PW})),
    );
    assert_eq!(signed_in.status(), 200);
    let s3 = cookie_of(&signed_in, "vy_session").unwrap();
    let setup = json_body(http(
        ureq::post(format!("{base2}/api/v1/auth/mfa/setup").as_str())
            .set("cookie", &s3)
            .call(),
    ));
    let secret2 = setup["secret"].as_str().unwrap().to_owned();
    let on = http(
        ureq::post(format!("{base2}/api/v1/auth/mfa/confirm").as_str())
            .set("cookie", &s3)
            .send_json(json!({"code": code_for(&secret2)})),
    );
    assert_eq!(on.status(), 200);
    let forged = forge_challenge(me["id"].as_i64().unwrap());
    let forged_try = http(
        ureq::post(format!("{base2}/api/v1/auth/mfa").as_str())
            .send_json(json!({"challenge": forged, "code": code_for(&secret2)})),
    );
    assert_eq!(forged_try.status(), 401, "a forged challenge is refused");
    assert!(cookie_of(&forged_try, "vy_session").is_none());

    // A genuine challenge still works — unless the account is suspended
    // between the password step and the code.
    let step1 = json_body(http(
        ureq::post(format!("{base2}/api/v1/auth/login").as_str())
            .send_json(json!({"email": "x@example.com", "password": PW})),
    ));
    let challenge = step1["challenge"].as_str().unwrap().to_owned();
    sqlx::query("UPDATE users SET suspended_at = now() WHERE email = 'x@example.com'")
        .execute(db.pool())
        .await
        .unwrap();
    let suspended = http(
        ureq::post(format!("{base2}/api/v1/auth/mfa").as_str())
            .send_json(json!({"challenge": challenge, "code": code_for(&secret2)})),
    );
    sqlx::query("UPDATE users SET suspended_at = NULL WHERE email = 'x@example.com'")
        .execute(db.pool())
        .await
        .unwrap();
    assert_eq!(suspended.status(), 401, "suspended mid-sign-in stays out");
    assert!(cookie_of(&suspended, "vy_session").is_none());
    // Nor may an account whose address stopped being confirmed in the
    // meantime finish with a code (phase 98).
    sqlx::query("UPDATE users SET email_verified_at = NULL WHERE email = 'x@example.com'")
        .execute(db.pool())
        .await
        .unwrap();
    let unconfirmed = http(
        ureq::post(format!("{base2}/api/v1/auth/mfa").as_str())
            .send_json(json!({"challenge": challenge, "code": code_for(&secret2)})),
    );
    sqlx::query("UPDATE users SET email_verified_at = now() WHERE email = 'x@example.com'")
        .execute(db.pool())
        .await
        .unwrap();
    assert!(cookie_of(&unconfirmed, "vy_session").is_none());
    // The generic answer of this step: the same as a forged challenge's.
    let forged_again = http(
        ureq::post(format!("{base2}/api/v1/auth/mfa").as_str())
            .send_json(json!({"challenge": forged, "code": code_for(&secret2)})),
    );
    assert_eq!(
        unconfirmed.status(),
        401,
        "unconfirmed mid-sign-in stays out"
    );
    assert_eq!(json_body(unconfirmed), json_body(forged_again));
    let fine = http(
        ureq::post(format!("{base2}/api/v1/auth/mfa").as_str())
            .send_json(json!({"challenge": challenge, "code": code_for(&secret2)})),
    );
    assert_eq!(fine.status(), 200, "the genuine challenge still signs in");
}

/// A challenge signed the way the server used to sign one when no
/// `VYASA_SECRET_KEY` was set.
fn forge_challenge(user_id: i64) -> String {
    use hmac::{Hmac, KeyInit, Mac};
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(b"vyasa/mfa-challenge/v1");
    h.update(b"vyasa-insecure");
    let key = h.finalize();
    let exp = chrono::Utc::now().timestamp() + 300;
    let payload = format!("{user_id}:{exp}");
    let mut mac = Hmac::<Sha256>::new_from_slice(&key).unwrap();
    mac.update(payload.as_bytes());
    format!("{payload}:{}", hex::encode(mac.finalize().into_bytes()))
}
