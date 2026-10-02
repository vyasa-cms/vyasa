//! End-to-end auth tests: boot the real `vyasa serve` binary against
//! the test database and exercise login → me → logout over HTTP.
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod common;

use std::io::Read;

use serde_json::{json, Value};
use sqlx::PgPool;
use vyasa_db::models::Role;
use vyasa_db::repo::{NewUser, UsersRepo};
use vyasa_testkit::{TestDb, TestServer};

/// Seeds the fixed test user through the repository layer.
async fn seed_user(pool: &PgPool) {
    let users = UsersRepo::new(pool.clone());
    let hash = vyasa_core::user::password::hash_password("correct horse").expect("hash");
    users
        .insert(&NewUser {
            id: 60_001,
            email: "auth-test@example.com",
            username: "authtest",
            display_name: "Auth Test",
            password_hash: Some(&hash),
            role: Role::Author,
            bio: "",
        })
        .await
        .expect("seed user");
}

/// Reads the whole body of a response as JSON.
fn json_body(response: ureq::Response) -> Value {
    let mut body = String::new();
    response
        .into_reader()
        .take(1_000_000)
        .read_to_string(&mut body)
        .expect("read body");
    serde_json::from_str(&body).expect("parse json body")
}

/// Executes a request, returning the response even for 4xx/5xx (ureq
/// wraps non-2xx in `Error::Status`); panics only on transport failures.
fn http(result: Result<ureq::Response, ureq::Error>) -> ureq::Response {
    match result {
        Ok(response) | Err(ureq::Error::Status(_, response)) => response,
        Err(ureq::Error::Transport(err)) => panic!("transport failure: {err}"),
    }
}

#[tokio::test]
async fn login_me_logout_flow() {
    let db = TestDb::new().await;
    seed_user(db.pool()).await;
    let server = TestServer::start(common::BIN, &db);
    let base = server.base();

    // 1. me without a cookie -> 401 with the machine code.
    let me = http(ureq::get(format!("{base}/api/v1/auth/me").as_str()).call());
    assert_eq!(me.status(), 401);
    assert_eq!(json_body(me)["code"], "unauthorized");

    // 2. wrong password -> 401, no cookie set.
    let bad = http(
        ureq::post(format!("{base}/api/v1/auth/login").as_str())
            .send_json(json!({"email": "auth-test@example.com", "password": "wrong"})),
    );
    assert_eq!(bad.status(), 401);
    assert!(bad.all("set-cookie").is_empty());
    let bad_body = json_body(bad);

    // 3. unknown email -> identical failure (no user enumeration).
    let unknown = http(
        ureq::post(format!("{base}/api/v1/auth/login").as_str())
            .send_json(json!({"email": "ghost@example.com", "password": "wrong"})),
    );
    assert_eq!(unknown.status(), 401);
    let unknown_body = json_body(unknown);
    assert_eq!(unknown_body, bad_body, "responses must not differ");

    // 4. correct login (mixed-case email; citext folds) -> 200 + cookie.
    let login = http(
        ureq::post(format!("{base}/api/v1/auth/login").as_str()).send_json(json!({
            "email": "Auth-Test@Example.com",
            "password": "correct horse"
        })),
    );
    assert_eq!(login.status(), 200);
    let set_cookie = login.all("set-cookie").join("; ");
    assert!(set_cookie.contains("vy_session="), "cookie: {set_cookie}");
    assert!(set_cookie.contains("HttpOnly"), "cookie: {set_cookie}");
    assert!(set_cookie.contains("SameSite=Lax"), "cookie: {set_cookie}");
    // Plain http to the loopback (a developer's laptop) must not get a
    // Secure cookie: a browser silently drops one on an http origin and
    // nobody can log in.
    assert!(!set_cookie.contains("Secure"), "over http: {set_cookie}");
    let user = json_body(login);

    // 4b. The same login as the proxy presents it, over TLS -> Secure.
    // The old test was `bind_addr.starts_with("https://")`; a socket
    // address is never a URL, so it was never true and the cookie went out
    // without Secure on every deployment there has ever been.
    let over_tls = http(
        ureq::post(format!("{base}/api/v1/auth/login").as_str())
            .set("X-Forwarded-Proto", "https")
            .send_json(json!({
                "email": "auth-test@example.com",
                "password": "correct horse"
            })),
    );
    assert_eq!(over_tls.status(), 200);
    let tls_cookie = over_tls.all("set-cookie").join("; ");
    assert!(tls_cookie.contains("; Secure"), "over https: {tls_cookie}");
    assert_eq!(user["email"], "auth-test@example.com");
    assert_eq!(user["role"], "author");
    let token = set_cookie
        .split(';')
        .next()
        .and_then(|kv| kv.split('=').nth(1))
        .expect("cookie value")
        .to_string();

    // 5. me with the cookie -> same user.
    let me = http(
        ureq::get(format!("{base}/api/v1/auth/me").as_str())
            .set("cookie", &format!("vy_session={token}"))
            .call(),
    );
    assert_eq!(me.status(), 200);
    let me_body = json_body(me);
    assert_eq!(me_body["id"], user["id"]);

    // 6. logout revokes; the old cookie no longer authenticates.
    let logout = http(
        ureq::post(format!("{base}/api/v1/auth/logout").as_str())
            .set("cookie", &format!("vy_session={token}"))
            .call(),
    );
    assert_eq!(logout.status(), 200);
    let me = http(
        ureq::get(format!("{base}/api/v1/auth/me").as_str())
            .set("cookie", &format!("vy_session={token}"))
            .call(),
    );
    assert_eq!(me.status(), 401, "revoked session must not authenticate");

    // 7. malformed JSON -> 4xx without crashing the server.
    let bad_json = http(
        ureq::post(format!("{base}/api/v1/auth/login").as_str())
            .set("content-type", "application/json")
            .send_string("{not json"),
    );
    assert!(bad_json.status() >= 400);
    let alive = http(ureq::get(format!("{base}/api/v1/auth/me").as_str()).call());
    assert_eq!(alive.status(), 401, "server must survive malformed input");

    reset_links_ignore_the_request_host(base, db.pool()).await;
    password_change_needs_the_current_password(base);
    // Last: it spends this address's sign-in budget for the next minute.
    forwarded_for_does_not_buy_fresh_buckets(base);
}

fn login_cookie(base: &str, password: &str) -> String {
    let r = http(
        ureq::post(format!("{base}/api/v1/auth/login").as_str())
            .send_json(json!({"email": "auth-test@example.com", "password": password})),
    );
    assert_eq!(r.status(), 200, "login with {password}");
    r.all("set-cookie")
        .iter()
        .filter_map(|c| c.split(';').next())
        .find(|kv| kv.starts_with("vy_session="))
        .expect("session cookie")
        .to_owned()
}

/// A reset link used to be built from the request's `Host` when no site
/// address was configured, so a forged host got the victim a genuine
/// token on a link to the attacker's server.
async fn reset_links_ignore_the_request_host(base: &str, pg: &sqlx::PgPool) {
    let options = vyasa_db::repo::OptionsRepo::new(pg.clone());
    options.set("smtp_host", &json!("127.0.0.1")).await.unwrap();
    options.set("smtp_port", &json!("1")).await.unwrap();
    options.set("site_url", &json!("")).await.unwrap();
    let mails = || async {
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT payload->>'body' FROM jobs WHERE kind = 'send_email'
             AND payload->>'to' = 'auth-test@example.com'",
        )
        .fetch_all(pg)
        .await
        .unwrap();
        rows.into_iter().map(|r| r.0).collect::<Vec<_>>()
    };
    let forgot = || {
        http(
            ureq::post(format!("{base}/api/v1/auth/forgot").as_str())
                .set("Host", "evil.example")
                .set("X-Forwarded-Proto", "https")
                .send_json(json!({"email": "auth-test@example.com"})),
        )
        .status()
    };
    sqlx::query("DELETE FROM jobs WHERE kind = 'send_email'")
        .execute(pg)
        .await
        .unwrap();

    // No site address: nothing is sent rather than a link to evil.example.
    assert_eq!(forgot(), 200, "still always 200");
    assert!(mails().await.is_empty(), "no site_url, no mail");

    // With one: the link is on it, whatever Host the request carried.
    options
        .set("site_url", &json!("https://site.example"))
        .await
        .unwrap();
    assert_eq!(forgot(), 200);
    let sent = mails().await;
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert!(
        sent[0].contains("https://site.example/admin/reset?token="),
        "{}",
        sent[0]
    );
    assert!(!sent[0].contains("evil.example"), "{}", sent[0]);
    options.set("smtp_host", &json!("")).await.unwrap();
}

/// `PUT /users/me {password}` used to change the password on the strength
/// of the session alone and leave every other session signed in.
fn password_change_needs_the_current_password(base: &str) {
    let mine = login_cookie(base, "correct horse");
    let other = login_cookie(base, "correct horse");
    let put = |body: Value| {
        http(
            ureq::put(format!("{base}/api/v1/users/me").as_str())
                .set("cookie", &mine)
                .send_json(body),
        )
    };
    let missing = put(json!({"password": "a-new-password"}));
    assert_eq!(missing.status(), 400);
    assert!(json_body(missing)["message"]
        .as_str()
        .unwrap_or_default()
        .contains("current_password"));
    let wrong = put(json!({"password": "a-new-password", "current_password": "nope"}));
    assert_eq!(wrong.status(), 403);
    // A profile edit without a password change needs nothing extra.
    assert_eq!(put(json!({"bio": "hi"})).status(), 204);
    let ok = put(json!({"password": "a-new-password", "current_password": "correct horse"}));
    assert_eq!(ok.status(), 204);
    let me = |cookie: &str| {
        http(
            ureq::get(format!("{base}/api/v1/auth/me").as_str())
                .set("cookie", cookie)
                .call(),
        )
        .status()
    };
    assert_eq!(me(&mine), 200, "the session that changed it stays");
    assert_eq!(me(&other), 401, "every other session is ended");
    let _ = login_cookie(base, "a-new-password");
}

/// With no trusted proxy configured, the socket peer is the client: a
/// fresh `X-Forwarded-For` per request used to buy a fresh rate-limit
/// bucket, and so unlimited password guesses.
fn forwarded_for_does_not_buy_fresh_buckets(base: &str) {
    let mut limited = false;
    for i in 0..12 {
        let r = http(
            ureq::post(format!("{base}/api/v1/auth/login").as_str())
                .set("X-Forwarded-For", &format!("198.51.100.{i}"))
                .set("CF-Connecting-IP", &format!("203.0.113.{i}"))
                .send_json(json!({"email": "auth-test@example.com", "password": "wrong"})),
        );
        if r.status() == 429 {
            limited = true;
            break;
        }
    }
    assert!(
        limited,
        "spoofed forwarding headers must not dodge the limit"
    );
}
