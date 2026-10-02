//! Public registration over HTTP (phase 98): the four public routes, the
//! sign-in refusal, the limits, the honeypot, the mail each outcome sends,
//! the administrator's confirm and resend, and the site-health warning.
//!
//! Every server here trusts the loopback as a proxy, so a test can speak
//! from as many client addresses as it needs with `X-Forwarded-For`; the
//! per-address limits are the thing under test in one of them and would
//! otherwise get in the way of all the others.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::too_many_lines)]

mod common;

use std::sync::atomic::{AtomicU32, Ordering};

use serde_json::{json, Value};
use sqlx::PgPool;
use vyasa_db::models::{Role, TokenPurpose};
use vyasa_db::repo::{NewRole, NewUser, OptionsRepo, RolesRepo, TokensRepo, UsersRepo};
use vyasa_testkit::{TestDb, TestServer};

const SITE: &str = "https://site.example";
const SITE_TITLE: &str = "Lantern Club";
const PASSWORD: &str = "a-long-password";

/// A fresh client address per call, so the per-address limits only bite
/// where a test means them to.
fn next_ip() -> String {
    static NEXT: AtomicU32 = AtomicU32::new(1);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    format!("198.51.{}.{}", n / 250, n % 250 + 1)
}

fn http(r: Result<ureq::Response, ureq::Error>) -> ureq::Response {
    match r {
        Ok(v) | Err(ureq::Error::Status(_, v)) => v,
        Err(ureq::Error::Transport(e)) => panic!("transport {e}"),
    }
}

fn server(db: &TestDb) -> TestServer {
    TestServer::builder(common::BIN, db)
        .env("VYASA_TRUSTED_PROXIES", "127.0.0.1,::1")
        .start()
}

/// Status and parsed body (`Null` for an empty or non-JSON body).
fn outcome(r: ureq::Response) -> (u16, Value) {
    let status = r.status();
    let body = r.into_string().unwrap_or_default();
    (status, serde_json::from_str(&body).unwrap_or(Value::Null))
}

fn post_from(base: &str, ip: &str, path: &str, body: &Value) -> ureq::Response {
    http(
        ureq::post(&format!("{base}/api/v1{path}"))
            .set("X-Forwarded-For", ip)
            .send_json(body.clone()),
    )
}

fn post(base: &str, path: &str, body: &Value) -> ureq::Response {
    post_from(base, &next_ip(), path, body)
}

fn register_body(email: &str, name: &str, password: &str) -> Value {
    json!({ "email": email, "display_name": name, "password": password, "website": "" })
}

fn register(base: &str, email: &str, name: &str, password: &str) -> (u16, Value) {
    outcome(post(
        base,
        "/auth/register",
        &register_body(email, name, password),
    ))
}

fn resend(base: &str, email: &str) -> (u16, Value) {
    outcome(post(
        base,
        "/auth/register/resend",
        &json!({ "email": email }),
    ))
}

fn login(base: &str, ip: &str, email: &str, password: &str) -> (u16, Value, bool) {
    let r = post_from(
        base,
        ip,
        "/auth/login",
        &json!({ "email": email, "password": password }),
    );
    let cookie = r
        .all("set-cookie")
        .iter()
        .any(|c| c.starts_with("vy_session=") && !c.starts_with("vy_session=;"));
    let (status, body) = outcome(r);
    (status, body, cookie)
}

/// Registration on, a relay nothing listens on (mail is queued, never
/// delivered), and a site address and title for the links and wording.
async fn open(pool: &PgPool) {
    let options = OptionsRepo::new(pool.clone());
    for (key, value) in [
        ("registration_enabled", json!(true)),
        ("smtp_host", json!("127.0.0.1")),
        ("smtp_port", json!("1")),
        ("site_url", json!(SITE)),
        ("site_title", json!(SITE_TITLE)),
    ] {
        options.set(key, &value).await.unwrap();
    }
}

async fn set_option(pool: &PgPool, key: &str, value: Value) {
    OptionsRepo::new(pool.clone())
        .set(key, &value)
        .await
        .unwrap();
}

/// Every queued mail to `to` (any case), oldest first, as
/// (template, subject, body).
async fn mails_to(pool: &PgPool, to: &str) -> Vec<(String, String, String)> {
    sqlx::query_as(
        "SELECT payload->>'template', payload->>'subject', payload->>'body' FROM jobs
         WHERE kind = 'send_email' AND lower(payload->>'to') = lower($1)
         ORDER BY created_at, id",
    )
    .bind(to.trim())
    .fetch_all(pool)
    .await
    .unwrap()
}

async fn mail_count(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind = 'send_email'")
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn user_count(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM users")
        .fetch_one(pool)
        .await
        .unwrap()
}

/// The raw token in the first link in `body` that starts with `prefix`.
fn token_in(body: &str, prefix: &str) -> String {
    let start = body
        .find(prefix)
        .unwrap_or_else(|| panic!("{prefix} in {body}"))
        + prefix.len();
    body[start..]
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect()
}

/// An account as registration leaves it: unconfirmed, with `password`.
async fn unconfirmed(pool: &PgPool, id: i64, email: &str, role: Role) -> i64 {
    let hash = vyasa_core::user::password::hash_password(PASSWORD).unwrap();
    let username = format!("pending-{id}");
    UsersRepo::new(pool.clone())
        .insert_unconfirmed(
            &NewUser {
                id,
                email,
                username: &username,
                display_name: &username,
                password_hash: Some(&hash),
                role,
                bio: "",
            },
            None,
        )
        .await
        .map_err(|_| "insert")
        .unwrap();
    id
}

/// (email_verified_at is set, password hash, updated_at) of an account.
async fn account_state(pool: &PgPool, email: &str) -> Option<(bool, Option<String>, String)> {
    sqlx::query_as(
        "SELECT email_verified_at IS NOT NULL, password_hash, updated_at::text
         FROM users WHERE email = $1",
    )
    .bind(email)
    .fetch_optional(pool)
    .await
    .unwrap()
}

/// Audit actions recorded against `target`, waiting briefly for the
/// fire-and-forget writes.
async fn audit_actions(pool: &PgPool, target: &str, want: &[&str]) -> Vec<String> {
    let mut actions = Vec::new();
    for _ in 0..50 {
        actions = sqlx::query_scalar("SELECT action FROM audit_log WHERE target = $1")
            .bind(target)
            .fetch_all(pool)
            .await
            .unwrap();
        if want.iter().all(|w| actions.iter().any(|a| a == w)) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    actions
}

#[tokio::test]
async fn registration_is_off_until_an_administrator_turns_it_on() {
    let db = TestDb::new().await;
    let pool = db.pool();
    open(pool).await;
    set_option(pool, "registration_enabled", json!(false)).await;
    let server = server(&db);
    let base = server.base();

    let status = outcome(http(
        ureq::get(&format!("{base}/api/v1/auth/registration")).call(),
    ));
    assert_eq!(
        status,
        (
            200,
            json!({ "enabled": false, "available": false, "password_min_length": 8 })
        )
    );
    let (code, body) = register(base, "someone@example.com", "Some One", PASSWORD);
    assert_eq!(code, 403, "{body}");
    assert_eq!(body["code"], "registration_closed", "{body}");
    let (code, body) = resend(base, "someone@example.com");
    assert_eq!(code, 403, "{body}");
    assert_eq!(body["code"], "registration_closed", "{body}");
    assert_eq!(user_count(pool).await, 0);
    assert_eq!(mail_count(pool).await, 0);

    set_option(pool, "registration_enabled", json!(true)).await;
    let status = outcome(http(
        ureq::get(&format!("{base}/api/v1/auth/registration")).call(),
    ));
    assert_eq!(status.1["enabled"], true);
    assert_eq!(
        status.1["available"], true,
        "on, with a relay and an address"
    );
}

#[tokio::test]
async fn without_a_relay_or_a_site_address_nothing_is_created() {
    let db = TestDb::new().await;
    let pool = db.pool();
    open(pool).await;
    set_option(pool, "smtp_host", json!("")).await;
    let server = server(&db);
    let base = server.base();

    let available = || {
        outcome(http(
            ureq::get(&format!("{base}/api/v1/auth/registration")).call(),
        ))
        .1
    };
    for step in ["no relay", "no site address"] {
        // On, but it cannot work: the form is not offered.
        let info = available();
        assert_eq!(
            (info["enabled"].clone(), info["available"].clone()),
            (json!(true), json!(false)),
            "{step}: {info}"
        );
        let (code, body) = register(base, "someone@example.com", "Some One", PASSWORD);
        assert_eq!(code, 503, "{step}: {body}");
        assert_eq!(body["code"], "registration_unavailable", "{step}: {body}");
        let (code, body) = resend(base, "someone@example.com");
        assert_eq!(code, 503, "{step}: {body}");
        assert_eq!(body["code"], "registration_unavailable", "{step}: {body}");
        assert_eq!(user_count(pool).await, 0, "{step}");
        assert_eq!(mail_count(pool).await, 0, "{step}");
        set_option(pool, "smtp_host", json!("127.0.0.1")).await;
        set_option(pool, "site_url", json!("  ")).await;
    }
}

#[tokio::test]
async fn a_registrant_confirms_by_mail_and_only_then_signs_in() {
    let db = TestDb::new().await;
    let pool = db.pool();
    open(pool).await;
    let server = server(&db);
    let base = server.base();

    // A forged Host must not reach the link: links come from site_url.
    let accepted = outcome(http(
        ureq::post(&format!("{base}/api/v1/auth/register"))
            .set("X-Forwarded-For", &next_ip())
            .set("Host", "evil.example")
            .set("X-Forwarded-Proto", "https")
            .send_json(register_body(
                " Newcomer@Example.com ",
                "Nia Newcomer",
                PASSWORD,
            )),
    ));
    assert_eq!(accepted.0, 202, "{}", accepted.1);
    assert!(accepted.1["message"].is_string(), "{}", accepted.1);

    let (id, username, verified): (i64, String, bool) = sqlx::query_as(
        "SELECT id, username, email_verified_at IS NOT NULL FROM users
         WHERE email = 'newcomer@example.com'",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    assert!(!verified);
    assert!(username.starts_with("nia-newcomer-"), "{username}");

    let mails = mails_to(pool, "newcomer@example.com").await;
    assert_eq!(mails.len(), 1, "{mails:?}");
    let (template, subject, body) = &mails[0];
    assert_eq!(template, "registration_confirm");
    assert!(subject.contains(SITE_TITLE), "{subject}");
    // What the registrant typed as a name is not put in front of the
    // mailbox's owner, who may not be the registrant.
    assert!(!body.contains("Nia Newcomer"), "{body}");
    assert!(body.contains(SITE_TITLE), "{body}");
    assert!(
        body.contains(&format!("{SITE}/admin/verify?token=")),
        "{body}"
    );
    assert!(!body.contains("evil.example"), "{body}");
    assert!(body.contains("If you did not create an account"), "{body}");
    assert!(body.contains("do not click the link"), "{body}");
    let token = token_in(body, "/admin/verify?token=");

    // Until confirmed, the right password is refused exactly as a wrong
    // one is: anything else would tell a stranger who just registered an
    // address whether it was new.
    let ip = next_ip();
    let wrong = login(base, &ip, "newcomer@example.com", "not-the-password");
    assert_eq!(wrong.0, 401, "{}", wrong.1);
    assert!(!wrong.2);
    let right = login(base, &ip, "newcomer@example.com", PASSWORD);
    assert_eq!(right, wrong, "no session, and no different answer");

    // Confirmed with the password chosen at sign-up, which is kept.
    let verify = |token: &str| {
        post(
            base,
            "/auth/verify",
            &json!({ "token": token, "password": PASSWORD }),
        )
        .status()
    };
    assert_eq!(verify(&token), 204);
    assert_eq!(verify(&token), 400, "a link works once");
    assert_eq!(verify("vy-not-a-token"), 400);

    let (code, _, cookie) = login(base, &ip, "newcomer@example.com", PASSWORD);
    assert_eq!(code, 200);
    assert!(cookie);

    let actions = audit_actions(
        pool,
        &format!("user:{id}"),
        &["user.register", "user.confirm"],
    )
    .await;
    assert!(actions.iter().any(|a| a == "user.register"), "{actions:?}");
    assert!(actions.iter().any(|a| a == "user.confirm"), "{actions:?}");
}

#[tokio::test]
async fn registering_never_reveals_whether_an_address_has_an_account() {
    let db = TestDb::new().await;
    let pool = db.pool();
    open(pool).await;
    let confirmed = common::seed_user(pool, Role::Subscriber).await;
    let suspended = common::seed_user(pool, Role::Author).await;
    sqlx::query("UPDATE users SET suspended_at = now() WHERE id = $1")
        .bind(suspended.id)
        .execute(pool)
        .await
        .unwrap();
    unconfirmed(pool, 98_101, "pending@example.com", Role::Subscriber).await;
    let server = server(&db);
    let base = server.base();

    let untouched = [confirmed.email.clone(), suspended.email.clone()];
    let mut before = Vec::new();
    for email in &untouched {
        before.push(account_state(pool, email).await);
    }

    let addresses = [
        ("new", "brand-new@example.com".to_owned()),
        ("confirmed", confirmed.email.clone()),
        ("unconfirmed", "pending@example.com".to_owned()),
        ("suspended", suspended.email.clone()),
    ];
    let mut answers = Vec::new();
    for (kind, email) in &addresses {
        let answer = register(base, email, "Probe Name", "probe-password-1");
        answers.push((*kind, "register", answer));
        let answer = resend(base, email);
        answers.push((*kind, "resend", answer));
    }
    let (_, _, first) = &answers[0];
    assert_eq!(first.0, 202, "{first:?}");
    for (kind, route, answer) in &answers {
        assert_eq!(answer, first, "{kind} {route}");
    }
    // A malformed address to resend is the same answer too.
    assert_eq!(&resend(base, "not an address"), first);

    // What each address was sent differs, which only its owner can see.
    let template_of =
        |mails: Vec<(String, String, String)>| mails.into_iter().map(|m| m.0).collect::<Vec<_>>();
    assert_eq!(
        template_of(mails_to(pool, "brand-new@example.com").await),
        ["registration_confirm", "registration_confirm"]
    );
    assert_eq!(
        template_of(mails_to(pool, "pending@example.com").await),
        ["registration_confirm", "registration_confirm"]
    );
    for email in &untouched {
        let mails = mails_to(pool, email).await;
        assert_eq!(
            template_of(mails.clone()),
            ["registration_existing"],
            "{email}"
        );
        assert!(
            mails[0].2.contains(&format!("{SITE}/admin/login")),
            "{mails:?}"
        );
    }

    // Signing in with the password just sent says nothing either.
    let mut sign_ins = Vec::new();
    for (kind, email) in &addresses {
        let answer = login(base, &next_ip(), email, "probe-password-1");
        sign_ins.push((*kind, answer));
    }
    let (_, first_sign_in) = &sign_ins[0];
    assert_eq!(first_sign_in.0, 401, "{first_sign_in:?}");
    for (kind, answer) in &sign_ins {
        assert_eq!(answer, first_sign_in, "{kind} sign-in");
    }

    // Nothing about an existing account changed.
    for (email, was) in untouched.iter().zip(before) {
        assert_eq!(account_state(pool, email).await, was, "{email}");
    }

    // Nothing that follows differs either: no author page under the name
    // sent, and the same name registers again from another new address.
    let page = http(ureq::get(&format!("{base}/author/probe-name")).call());
    assert_eq!(page.status(), 404);
    assert_eq!(
        &register(
            base,
            "another-new@example.com",
            "Probe Name",
            "probe-password-1"
        ),
        first
    );
    let page = http(ureq::get(&format!("{base}/author/probe-name")).call());
    assert_eq!(page.status(), 404);
}

#[tokio::test]
async fn a_filled_honeypot_is_thanked_and_ignored() {
    let db = TestDb::new().await;
    let pool = db.pool();
    open(pool).await;
    let server = server(&db);
    let base = server.base();

    let genuine = register(base, "real@example.com", "Real", PASSWORD);
    let mut bot = register_body("bot@example.com", "Bot", PASSWORD);
    bot["website"] = json!("https://spam.example");
    let caught = outcome(post(base, "/auth/register", &bot));
    assert_eq!(caught, genuine);
    assert!(account_state(pool, "bot@example.com").await.is_none());
    assert!(mails_to(pool, "bot@example.com").await.is_empty());
}

#[tokio::test]
async fn one_client_address_registers_five_times_an_hour() {
    let db = TestDb::new().await;
    let pool = db.pool();
    open(pool).await;
    let server = server(&db);
    let base = server.base();

    let ip = next_ip();
    for n in 1..=5 {
        let r = post_from(
            base,
            &ip,
            "/auth/register",
            &register_body(&format!("burst{n}@example.com"), "Burst", PASSWORD),
        );
        assert_eq!(r.status(), 202, "registration {n}");
    }
    let (code, body) = outcome(post_from(
        base,
        &ip,
        "/auth/register",
        &register_body("burst6@example.com", "Burst", PASSWORD),
    ));
    assert_eq!(code, 429, "{body}");
    assert_eq!(body["code"], "rate_limited");
    assert!(account_state(pool, "burst6@example.com").await.is_none());
    // Another address is not held back by that one.
    assert_eq!(
        register(base, "burst6@example.com", "Burst", PASSWORD).0,
        202
    );
}

/// An IPv6 client can rotate through its whole /64: every address in it
/// shares one registration bucket.
#[tokio::test]
async fn one_ipv6_slash_64_registers_five_times_an_hour() {
    let db = TestDb::new().await;
    let pool = db.pool();
    open(pool).await;
    let server = server(&db);
    let base = server.base();

    for n in 1..=5 {
        let r = post_from(
            base,
            &format!("2001:db8:98:1::{n:x}"),
            "/auth/register",
            &register_body(&format!("v6-{n}@example.com"), "Six", PASSWORD),
        );
        assert_eq!(r.status(), 202, "registration {n}");
    }
    let (code, body) = outcome(post_from(
        base,
        "2001:db8:98:1:ffff:ffff:ffff:fffe",
        "/auth/register",
        &register_body("v6-6@example.com", "Six", PASSWORD),
    ));
    assert_eq!(code, 429, "{body}");
    assert_eq!(body["code"], "rate_limited");
    assert!(account_state(pool, "v6-6@example.com").await.is_none());
    // The next /64 is another client.
    let r = post_from(
        base,
        "2001:db8:98:2::1",
        "/auth/register",
        &register_body("v6-6@example.com", "Six", PASSWORD),
    );
    assert_eq!(r.status(), 202);
}

#[tokio::test]
async fn an_address_gets_three_confirmation_mails_an_hour() {
    let db = TestDb::new().await;
    let pool = db.pool();
    open(pool).await;
    let server = server(&db);
    let base = server.base();

    // Per address, whichever route and client address ask.
    let (first, _) = register(base, "limited@example.com", "Limited", PASSWORD);
    assert_eq!(first, 202);
    assert_eq!(resend(base, "limited@example.com").0, 202);
    assert_eq!(resend(base, "LIMITED@example.com").0, 202);
    assert_eq!(resend(base, "limited@example.com").0, 202, "still 202");
    // A fourth with the same password sends and issues nothing.
    assert_eq!(
        register(base, "limited@example.com", "Limited", PASSWORD).0,
        202
    );
    assert_eq!(mails_to(pool, "limited@example.com").await.len(), 3);
    let (_, hash, _) = account_state(pool, "limited@example.com").await.unwrap();
    assert!(hash.is_some(), "the same password again is no contest");
    let tokens: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM reset_tokens t JOIN users u ON u.id = t.user_id
         WHERE u.email = 'limited@example.com' AND t.purpose = 'verify'",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(tokens, 3, "no token for a suppressed mail");

    // Per account, from the database: links issued before this process
    // started (another node, a restart) count too.
    let id = unconfirmed(pool, 98_201, "earlier@example.com", Role::Subscriber).await;
    let repo = TokensRepo::new(pool.clone());
    for n in 0..3 {
        repo.issue(
            id,
            TokenPurpose::Verify,
            &format!("earlier-{n}"),
            chrono::Duration::hours(24),
        )
        .await
        .unwrap();
    }
    assert_eq!(resend(base, "earlier@example.com").0, 202);
    assert_eq!(
        register(base, "earlier@example.com", "Earlier", "different-password").0,
        202
    );
    assert!(mails_to(pool, "earlier@example.com").await.is_empty());
    let issued: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM reset_tokens WHERE user_id = $1 AND purpose = 'verify'",
    )
    .bind(id)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(issued, 3, "no link for a suppressed request");
    // The contest still happens without the mail: a different password
    // takes the stored one away.
    let (_, hash, _) = account_state(pool, "earlier@example.com").await.unwrap();
    assert!(hash.is_none(), "the contested password was cleared");
}

#[tokio::test]
async fn a_contested_registration_confirms_into_a_set_your_password_mail() {
    let db = TestDb::new().await;
    let pool = db.pool();
    open(pool).await;
    let server = server(&db);
    let base = server.base();

    assert_eq!(register(base, "shared@example.com", "Sam", PASSWORD).0, 202);
    assert_eq!(
        register(base, "shared@example.com", "Sam", "someone-elses-pw").0,
        202
    );
    let (_, hash, _) = account_state(pool, "shared@example.com").await.unwrap();
    assert!(hash.is_none(), "two passwords: neither stands");
    let mails = mails_to(pool, "shared@example.com").await;
    assert_eq!(mails.len(), 2);
    assert!(
        mails[1].2.contains("set a password"),
        "the repeat says a password will be needed: {}",
        mails[1].2
    );
    let token = token_in(&mails[1].2, "/admin/verify?token=");
    assert_eq!(
        post(base, "/auth/verify", &json!({ "token": token })).status(),
        204,
        "the same answer as any confirmation"
    );

    let mails = mails_to(pool, "shared@example.com").await;
    assert_eq!(mails.len(), 3, "{mails:?}");
    let (template, _, body) = &mails[2];
    assert_eq!(template, "password_set");
    assert!(
        body.contains(&format!("{SITE}/admin/reset?token=")),
        "{body}"
    );
    assert!(!body.contains("invited"), "{body}");
    for password in [PASSWORD, "someone-elses-pw"] {
        assert_eq!(
            login(base, &next_ip(), "shared@example.com", password).0,
            401
        );
    }

    // Asking for a reset of a passwordless account gets the same wording,
    // not an invitation from an administrator nobody saw.
    assert_eq!(
        post(
            base,
            "/auth/forgot",
            &json!({ "email": "shared@example.com" })
        )
        .status(),
        200
    );
    let mails = mails_to(pool, "shared@example.com").await;
    assert_eq!(mails.len(), 4);
    assert_eq!(mails[3].0, "password_set");
    let token = token_in(&mails[3].2, "/admin/reset?token=");
    assert_eq!(
        post(
            base,
            "/auth/reset",
            &json!({ "token": token, "password": "finally-mine-1" })
        )
        .status(),
        200
    );
    assert_eq!(
        login(base, &next_ip(), "shared@example.com", "finally-mine-1").0,
        200
    );
}

/// An unconfirmed account's right password is a failed attempt like any
/// other, lockout included: a lockout that behaved differently would be
/// the oracle the uniform answer closes.
#[tokio::test]
async fn an_unconfirmed_sign_in_fails_exactly_like_a_wrong_password() {
    let db = TestDb::new().await;
    let pool = db.pool();
    let id = unconfirmed(pool, 98_301, "waiting@example.com", Role::Subscriber).await;
    let confirmed = common::seed_user(pool, Role::Subscriber).await;
    let server = server(&db);
    let base = server.base();

    let (pending_ip, confirmed_ip) = (next_ip(), next_ip());
    let mut pending = Vec::new();
    let mut wrong = Vec::new();
    for _ in 1..=6 {
        pending.push(login(base, &pending_ip, "waiting@example.com", PASSWORD));
        wrong.push(login(
            base,
            &confirmed_ip,
            &confirmed.email,
            "not-the-password",
        ));
    }
    assert_eq!(pending, wrong);
    let last = &pending[5].1["message"];
    assert!(
        last.as_str().unwrap().contains("too many failed attempts"),
        "the lockout engaged: {last}"
    );
    // Confirmed (as a link opened with this password does), the account
    // signs in from another client address.
    sqlx::query("UPDATE users SET email_verified_at = now() WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await
        .unwrap();
    assert_eq!(
        login(base, &next_ip(), "waiting@example.com", PASSWORD).0,
        200
    );
}

/// Someone who knows an address registers it first and spends its
/// confirmation-mail budget; the owner's own registration, suppressed by
/// that budget, must still take the squatter's password away.
#[tokio::test]
async fn an_exhausted_mail_budget_does_not_let_a_squatters_password_stand() {
    let db = TestDb::new().await;
    let pool = db.pool();
    open(pool).await;
    let server = server(&db);
    let base = server.base();

    for _ in 0..3 {
        assert_eq!(
            register(base, "victim@example.com", "Squatter", "squatter-pw-1").0,
            202
        );
    }
    let mails = mails_to(pool, "victim@example.com").await;
    assert_eq!(mails.len(), 3);
    let squatters_link = token_in(&mails[0].2, "/admin/verify?token=");
    // The victim registers; the mail budget is spent, so nothing is sent.
    assert_eq!(
        register(base, "victim@example.com", "Victim", "victims-own-pw").0,
        202
    );
    assert_eq!(mails_to(pool, "victim@example.com").await.len(), 3);
    let (_, hash, _) = account_state(pool, "victim@example.com").await.unwrap();
    assert!(hash.is_none(), "the contested password is gone");
    // The victim confirms with a link the squatter had mailed to them.
    assert_eq!(
        post(base, "/auth/verify", &json!({ "token": squatters_link })).status(),
        204
    );
    assert_eq!(
        login(base, &next_ip(), "victim@example.com", "squatter-pw-1").0,
        401
    );
    assert_eq!(
        login(base, &next_ip(), "victim@example.com", "victims-own-pw").0,
        401
    );
}

/// The confirmation token of a fresh, uncontested
/// registration of `email` with [`PASSWORD`].
async fn registered(pool: &PgPool, base: &str, email: &str) -> String {
    assert_eq!(register(base, email, "Registrant", PASSWORD).0, 202);
    let mails = mails_to(pool, email).await;
    let (template, _, body) = mails.last().expect("a confirmation mail");
    assert_eq!(template, "registration_confirm");
    token_in(body, "/admin/verify?token=")
}

fn verify_with(base: &str, body: &Value) -> (u16, Value) {
    outcome(post(base, "/auth/verify", body))
}

/// One squatter registration is enough to set a password on someone
/// else's address; whoever opens the link (the owner, or a mail scanner
/// that follows it) must not turn that password into a working one.
#[tokio::test]
async fn a_confirmation_without_the_password_clears_it_and_mails_a_set_password_link() {
    let db = TestDb::new().await;
    let pool = db.pool();
    open(pool).await;
    let server = server(&db);
    let base = server.base();

    let token = registered(pool, base, "owner@example.com").await;
    assert_eq!(
        verify_with(base, &json!({ "token": token })),
        (204, Value::Null)
    );

    let (verified, hash, _) = account_state(pool, "owner@example.com").await.unwrap();
    assert!(verified);
    assert!(hash.is_none(), "the registrant's password did not survive");
    assert_eq!(
        login(base, &next_ip(), "owner@example.com", PASSWORD).0,
        401,
        "the password typed at sign-up does not sign in"
    );
    let mails = mails_to(pool, "owner@example.com").await;
    let (template, _, body) = mails.last().unwrap();
    assert_eq!(template, "password_set", "{mails:?}");
    let reset = token_in(body, "/admin/reset?token=");
    assert_eq!(
        post(
            base,
            "/auth/reset",
            &json!({ "token": reset, "password": "the-owners-own-1" })
        )
        .status(),
        200
    );
    assert_eq!(
        login(base, &next_ip(), "owner@example.com", "the-owners-own-1").0,
        200
    );
}

#[tokio::test]
async fn a_confirmation_with_the_password_chosen_at_sign_up_keeps_it() {
    let db = TestDb::new().await;
    let pool = db.pool();
    open(pool).await;
    let server = server(&db);
    let base = server.base();

    let token = registered(pool, base, "keeper@example.com").await;
    let (_, before, _) = account_state(pool, "keeper@example.com").await.unwrap();
    assert_eq!(
        verify_with(base, &json!({ "token": token, "password": PASSWORD })),
        (204, Value::Null),
        "the same answer as a confirmation that cleared the password"
    );
    let (verified, after, _) = account_state(pool, "keeper@example.com").await.unwrap();
    assert!(verified);
    assert_eq!(after, before);
    assert_eq!(
        login(base, &next_ip(), "keeper@example.com", PASSWORD).0,
        200
    );
    let templates: Vec<String> = mails_to(pool, "keeper@example.com")
        .await
        .into_iter()
        .map(|m| m.0)
        .collect();
    assert_eq!(templates, ["registration_confirm"], "no set-password mail");
}

#[tokio::test]
async fn a_wrong_password_at_confirmation_is_refused_and_leaves_the_link_working() {
    let db = TestDb::new().await;
    let pool = db.pool();
    open(pool).await;
    let server = server(&db);
    let base = server.base();

    let token = registered(pool, base, "typo@example.com").await;
    let before = account_state(pool, "typo@example.com").await;
    for _ in 0..3 {
        let (code, body) = verify_with(
            base,
            &json!({ "token": token, "password": "not-what-i-typed" }),
        );
        assert_eq!(code, 400, "{body}");
        assert_eq!(body["code"], "password_mismatch", "{body}");
    }
    assert_eq!(
        account_state(pool, "typo@example.com").await,
        before,
        "still unconfirmed, password untouched"
    );
    assert_eq!(login(base, &next_ip(), "typo@example.com", PASSWORD).0, 401);
    // The link was not spent: the right password still confirms with it.
    assert_eq!(
        verify_with(base, &json!({ "token": token, "password": PASSWORD })).0,
        204
    );
    assert_eq!(login(base, &next_ip(), "typo@example.com", PASSWORD).0, 200);

    // An account whose password a contest cleared has none to match.
    assert_eq!(register(base, "both@example.com", "A", PASSWORD).0, 202);
    assert_eq!(
        register(base, "both@example.com", "B", "another-pw-1").0,
        202
    );
    let mails = mails_to(pool, "both@example.com").await;
    let token = token_in(&mails[1].2, "/admin/verify?token=");
    for password in [PASSWORD, "another-pw-1"] {
        let (code, body) = verify_with(base, &json!({ "token": token, "password": password }));
        assert_eq!(
            (code, body["code"].clone()),
            (400, json!("password_mismatch"))
        );
    }
    assert_eq!(verify_with(base, &json!({ "token": token })).0, 204);
}

/// A reset link proves the mailbox as well as a confirmation link does,
/// and the password set through it is the owner's.
#[tokio::test]
async fn a_password_reset_confirms_an_unconfirmed_account() {
    let db = TestDb::new().await;
    let pool = db.pool();
    open(pool).await;
    let server = server(&db);
    let base = server.base();

    let token = registered(pool, base, "forgetful@example.com").await;
    assert_eq!(
        post(
            base,
            "/auth/forgot",
            &json!({ "email": "forgetful@example.com" })
        )
        .status(),
        200
    );
    let mails = mails_to(pool, "forgetful@example.com").await;
    let (template, _, body) = mails.last().unwrap();
    assert_eq!(template, "password_reset", "{mails:?}");
    let reset = token_in(body, "/admin/reset?token=");
    assert_eq!(
        post(
            base,
            "/auth/reset",
            &json!({ "token": reset, "password": "chosen-on-reset" })
        )
        .status(),
        200
    );
    let (verified, _, _) = account_state(pool, "forgetful@example.com").await.unwrap();
    assert!(verified, "the reset confirmed the address");
    assert_eq!(
        login(base, &next_ip(), "forgetful@example.com", "chosen-on-reset").0,
        200
    );
    assert_eq!(
        login(base, &next_ip(), "forgetful@example.com", PASSWORD).0,
        401
    );
    // Its confirmation links ended with the confirmation.
    assert_eq!(verify_with(base, &json!({ "token": token })).0, 400);
}

/// The administrator's "Confirm" vouches for the mailbox only.
#[tokio::test]
async fn an_administrators_confirmation_clears_the_password_and_mails_a_link() {
    let db = TestDb::new().await;
    let pool = db.pool();
    open(pool).await;
    let admin = common::seed_user(pool, Role::Admin).await;
    let pending = unconfirmed(pool, 98_501, "vouched@example.com", Role::Subscriber).await;
    let server = server(&db);
    let base = server.base();
    let cookie = common::login_cookie(base, &admin.email, &admin.password);

    let (code, body) = call(base, &cookie, "POST", &format!("/users/{pending}/confirm"));
    assert_eq!(code, 200, "{body}");
    assert_eq!(body["email_verified"], true);
    let (verified, hash, _) = account_state(pool, "vouched@example.com").await.unwrap();
    assert!(verified);
    assert!(hash.is_none(), "the password typed at sign-up is gone");
    assert_eq!(
        login(base, &next_ip(), "vouched@example.com", PASSWORD).0,
        401
    );
    let mails = mails_to(pool, "vouched@example.com").await;
    assert_eq!(mails.len(), 1, "{mails:?}");
    assert_eq!(mails[0].0, "password_set");
    let reset = token_in(&mails[0].2, "/admin/reset?token=");
    assert_eq!(
        post(
            base,
            "/auth/reset",
            &json!({ "token": reset, "password": "set-by-the-owner" })
        )
        .status(),
        200
    );
    assert_eq!(
        login(base, &next_ip(), "vouched@example.com", "set-by-the-owner").0,
        200
    );

    // Confirming an account that is confirmed already changes nothing and
    // sends nothing.
    let (code, _) = call(base, &cookie, "POST", &format!("/users/{pending}/confirm"));
    assert_eq!(code, 200);
    assert_eq!(mails_to(pool, "vouched@example.com").await.len(), 1);
    assert_eq!(
        login(base, &next_ip(), "vouched@example.com", "set-by-the-owner").0,
        200
    );

    // Without a way to mail the link, nothing is confirmed.
    let other = unconfirmed(pool, 98_502, "nomail@example.com", Role::Subscriber).await;
    set_option(pool, "smtp_host", json!("")).await;
    let (code, body) = call(base, &cookie, "POST", &format!("/users/{other}/confirm"));
    assert_eq!(code, 400, "{body}");
    let (verified, hash, _) = account_state(pool, "nomail@example.com").await.unwrap();
    assert!(!verified && hash.is_some(), "untouched");
}

/// The administrator learns when the set-password mail could not be
/// queued: by then the account is confirmed and its password gone, so
/// the person has no way in until someone sends a link.
#[tokio::test]
async fn an_administrators_confirmation_says_when_the_set_password_mail_failed() {
    let db = TestDb::new().await;
    let pool = db.pool();
    open(pool).await;
    let admin = common::seed_user(pool, Role::Admin).await;
    let mailed = unconfirmed(pool, 98_511, "mailed@example.com", Role::Subscriber).await;
    let unmailed = unconfirmed(pool, 98_512, "unmailed@example.com", Role::Subscriber).await;
    let server = server(&db);
    let base = server.base();
    let cookie = common::login_cookie(base, &admin.email, &admin.password);

    let (code, body) = call(base, &cookie, "POST", &format!("/users/{mailed}/confirm"));
    assert_eq!(code, 200, "{body}");
    assert_eq!(body["email_verified"], true);
    assert_eq!(body["link_sent"], true, "{body}");
    // Confirmed already: nothing was due, so nothing is said about mail.
    let (code, body) = call(base, &cookie, "POST", &format!("/users/{mailed}/confirm"));
    assert_eq!(code, 200, "{body}");
    assert!(body.get("link_sent").is_none(), "{body}");

    // The mail queue refuses (a trigger stands in for a database that
    // fails at that moment).
    for statement in [
        "CREATE FUNCTION refuse_mail() RETURNS trigger LANGUAGE plpgsql AS
         $$ BEGIN RAISE EXCEPTION 'mail queue down'; END $$",
        "CREATE TRIGGER refuse_mail BEFORE INSERT ON jobs FOR EACH ROW
         WHEN (NEW.kind = 'send_email') EXECUTE FUNCTION refuse_mail()",
    ] {
        sqlx::query(statement).execute(pool).await.unwrap();
    }
    let (code, body) = call(base, &cookie, "POST", &format!("/users/{unmailed}/confirm"));
    assert_eq!(code, 200, "{body}");
    assert_eq!(body["email_verified"], true);
    assert_eq!(body["has_password"], false);
    assert_eq!(body["link_sent"], false, "{body}");
    let (verified, hash, _) = account_state(pool, "unmailed@example.com").await.unwrap();
    assert!(verified && hash.is_none());
    assert!(mails_to(pool, "unmailed@example.com").await.is_empty());
}

/// Every part of a `/auth/forgot` answer a client could compare.
fn forgot_answer(base: &str, email: &str) -> (u16, String, String) {
    let r = post(base, "/auth/forgot", &json!({ "email": email }));
    let status = r.status();
    let content_type = r.header("content-type").unwrap_or_default().to_owned();
    (status, content_type, r.into_string().unwrap_or_default())
}

/// `/auth/forgot` used to mail an unconfirmed address as often as anyone
/// asked: it shares the registration mail limits now, and every address
/// has its own forgot limit, while the answer never changes.
#[tokio::test]
async fn forgot_for_an_unconfirmed_address_stops_mailing_at_the_registration_limit() {
    let db = TestDb::new().await;
    let pool = db.pool();
    open(pool).await;
    let server = server(&db);
    let base = server.base();

    registered(pool, base, "pending@example.com").await;
    let first = forgot_answer(base, "pending@example.com");
    assert_eq!(first.0, 200, "{first:?}");
    for n in 2..=6 {
        assert_eq!(
            forgot_answer(base, "Pending@Example.com "),
            first,
            "request {n}"
        );
    }
    // One confirmation and two resets: three mails an hour, whichever
    // route sent them.
    let templates: Vec<String> = mails_to(pool, "pending@example.com")
        .await
        .into_iter()
        .map(|m| m.0)
        .collect();
    assert_eq!(
        templates,
        ["registration_confirm", "password_reset", "password_reset"]
    );
    // The same answer for an address nobody has.
    assert_eq!(forgot_answer(base, "nobody@example.com"), first);
    // And the registration routes see the spent budget too.
    assert_eq!(resend(base, "pending@example.com").0, 202);
    assert_eq!(mails_to(pool, "pending@example.com").await.len(), 3);
}

#[tokio::test]
async fn forgot_mails_any_one_address_at_most_three_times_an_hour() {
    let db = TestDb::new().await;
    let pool = db.pool();
    open(pool).await;
    let member = common::seed_user(pool, Role::Subscriber).await;
    let other = common::seed_user(pool, Role::Subscriber).await;
    let server = server(&db);
    let base = server.base();

    let first = forgot_answer(base, &member.email);
    assert_eq!(first.0, 200);
    // However the address is spelt, it is one address.
    for n in 2..=6 {
        let spelling = match n % 3 {
            0 => member.email.to_uppercase(),
            1 => format!("  {} ", member.email),
            _ => member.email.clone(),
        };
        assert_eq!(forgot_answer(base, &spelling), first, "request {n}");
    }
    let mails = mails_to(pool, &member.email).await;
    assert_eq!(mails.len(), 3, "{mails:?}");
    assert!(mails.iter().all(|m| m.0 == "password_reset"));
    // Another address is not held back by that one.
    assert_eq!(forgot_answer(base, &other.email), first);
    assert_eq!(mails_to(pool, &other.email).await.len(), 1);
}

/// The display name on an account a registration created was typed by
/// whoever registered first; mail the address's owner gets does not show it.
#[tokio::test]
async fn reset_and_already_registered_mails_carry_no_display_name() {
    let db = TestDb::new().await;
    let pool = db.pool();
    open(pool).await;
    let server = server(&db);
    let base = server.base();

    assert_eq!(
        register(base, "named@example.com", "Mallory Typed This", PASSWORD).0,
        202
    );
    assert_eq!(forgot_answer(base, "named@example.com").0, 200);
    sqlx::query("UPDATE users SET email_verified_at = now() WHERE email = 'named@example.com'")
        .execute(pool)
        .await
        .unwrap();
    assert_eq!(
        register(base, "named@example.com", "Someone", PASSWORD).0,
        202
    );
    let mails = mails_to(pool, "named@example.com").await;
    let templates: Vec<&str> = mails.iter().map(|m| m.0.as_str()).collect();
    assert_eq!(
        templates,
        [
            "registration_confirm",
            "password_reset",
            "registration_existing"
        ]
    );
    for (template, subject, body) in &mails {
        assert!(
            !body.contains("Mallory") && !subject.contains("Mallory"),
            "{template}: {body}"
        );
    }
}

async fn seed_with_caps(pool: &PgPool, slug: &str, caps: &[&str]) -> common::Seeded {
    let capabilities: Vec<String> = caps.iter().map(|c| (*c).to_owned()).collect();
    RolesRepo::new(pool.clone())
        .insert(&NewRole {
            slug,
            name: slug,
            description: "",
            capabilities: &capabilities,
        })
        .await
        .unwrap();
    let user = common::seed_user(pool, Role::Subscriber).await;
    UsersRepo::new(pool.clone())
        .set_custom_role(user.id, Some(slug))
        .await
        .unwrap();
    user
}

fn call(base: &str, cookie: &str, method: &str, path: &str) -> (u16, Value) {
    outcome(http(
        ureq::request(method, &format!("{base}/api/v1{path}"))
            .set("cookie", cookie)
            // POST /users/* shares the tight sign-in bucket.
            .set("X-Forwarded-For", &next_ip())
            .call(),
    ))
}

#[tokio::test]
async fn a_user_manager_confirms_and_resends_within_reach() {
    let db = TestDb::new().await;
    let pool = db.pool();
    open(pool).await;
    let admin = common::seed_user(pool, Role::Admin).await;
    let manager = seed_with_caps(pool, "people", &["view_admin", "manage_users"]).await;
    let bystander = common::seed_user(pool, Role::Editor).await;
    let reader = unconfirmed(pool, 98_401, "reader@example.com", Role::Subscriber).await;
    let editor = unconfirmed(pool, 98_402, "editor@example.com", Role::Editor).await;
    let server = server(&db);
    let base = server.base();
    let manager_cookie = common::login_cookie(base, &manager.email, &manager.password);
    let editor_cookie = common::login_cookie(base, &bystander.email, &bystander.password);
    let admin_cookie = common::login_cookie(base, &admin.email, &admin.password);

    // Beyond reach: an account holding capabilities the manager lacks.
    for action in ["confirm", "resend-confirmation"] {
        let (code, body) = call(
            base,
            &manager_cookie,
            "POST",
            &format!("/users/{editor}/{action}"),
        );
        assert_eq!(code, 403, "{action}: {body}");
    }
    assert!(!account_state(pool, "editor@example.com").await.unwrap().0);
    assert!(mails_to(pool, "editor@example.com").await.is_empty());
    // Without manage_users at all.
    for action in ["confirm", "resend-confirmation"] {
        let (code, _) = call(
            base,
            &editor_cookie,
            "POST",
            &format!("/users/{reader}/{action}"),
        );
        assert_eq!(code, 403, "{action}");
    }
    assert_eq!(
        call(base, &manager_cookie, "POST", "/users/9999999/confirm").0,
        404
    );

    // Within reach: resend mails a fresh link, confirm confirms.
    let (code, body) = call(
        base,
        &manager_cookie,
        "POST",
        &format!("/users/{reader}/resend-confirmation"),
    );
    assert_eq!(code, 204, "{body}");
    let mails = mails_to(pool, "reader@example.com").await;
    assert_eq!(mails.len(), 1);
    assert_eq!(mails[0].0, "registration_confirm");
    assert!(mails[0].2.contains(&format!("{SITE}/admin/verify?token=")));

    let (code, body) = call(
        base,
        &manager_cookie,
        "POST",
        &format!("/users/{reader}/confirm"),
    );
    assert_eq!(code, 200, "{body}");
    assert_eq!(body["email_verified"], true);
    let (code, body) = call(
        base,
        &manager_cookie,
        "POST",
        &format!("/users/{reader}/resend-confirmation"),
    );
    assert_eq!(code, 400, "already confirmed: {body}");
    // The administrator vouched for the mailbox, not for the password a
    // registrant typed: it is gone, and the mailbox gets a link to set one.
    assert_eq!(
        login(base, &next_ip(), "reader@example.com", PASSWORD).0,
        401
    );
    let mails = mails_to(pool, "reader@example.com").await;
    assert_eq!(mails.last().unwrap().0, "password_set", "{mails:?}");

    // The users list says who is confirmed.
    let (code, list) = call(base, &admin_cookie, "GET", "/users?per_page=100");
    assert_eq!(code, 200);
    let verified = |id: i64| {
        list.as_array()
            .unwrap()
            .iter()
            .find(|u| u["id"] == id)
            .map(|u| u["email_verified"].clone())
    };
    assert_eq!(verified(reader), Some(json!(true)));
    assert_eq!(verified(editor), Some(json!(false)));
    assert_eq!(verified(admin.id), Some(json!(true)));

    // An administrator reaches everyone.
    let (code, _) = call(
        base,
        &admin_cookie,
        "POST",
        &format!("/users/{editor}/confirm"),
    );
    assert_eq!(code, 200);
    let actions = audit_actions(
        pool,
        &format!("user:{reader}"),
        &["user.confirm", "user.resend_confirmation"],
    )
    .await;
    assert!(actions.iter().any(|a| a == "user.confirm"), "{actions:?}");
    assert!(
        actions.iter().any(|a| a == "user.resend_confirmation"),
        "{actions:?}"
    );
}

/// A stranger gets at most an author's power: the editor role, and any
/// role that edits others' work, moderates comments or manages the shared
/// categories, is never the default.
#[tokio::test]
async fn an_editor_is_never_the_role_a_stranger_gets() {
    let db = TestDb::new().await;
    let pool = db.pool();
    open(pool).await;
    let admin = common::seed_user(pool, Role::Admin).await;
    for (slug, cap) in [
        ("others", "edit_others"),
        ("moderators", "moderate_comments"),
        ("taxonomists", "manage_categories"),
    ] {
        RolesRepo::new(pool.clone())
            .insert(&NewRole {
                slug,
                name: slug,
                description: "",
                capabilities: &["view_admin".to_owned(), cap.to_owned()],
            })
            .await
            .unwrap();
    }
    let server = server(&db);
    let base = server.base();
    let cookie = common::login_cookie(base, &admin.email, &admin.password);

    let put = |role: &str| {
        outcome(http(
            ureq::put(&format!("{base}/api/v1/options"))
                .set("cookie", &cookie)
                .set("X-Forwarded-For", &next_ip())
                .send_json(json!({ "registration_default_role": role })),
        ))
    };
    for refused in ["editor", "others", "moderators", "taxonomists"] {
        let (code, body) = put(refused);
        assert_eq!(code, 400, "{refused}: {body}");
        assert!(
            body["message"]
                .as_str()
                .unwrap()
                .contains("moderate comments"),
            "{body}"
        );
    }
    assert_eq!(put("author").0, 204);

    // Saved before the rule (or written past it): new accounts are
    // subscribers, and site health says so.
    set_option(pool, "registration_default_role", json!("editor")).await;
    assert_eq!(register(base, "stranger@example.com", "S", PASSWORD).0, 202);
    let role: String =
        sqlx::query_scalar("SELECT role FROM users WHERE email = 'stranger@example.com'")
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(role, "subscriber");
    // Likewise a role over the shared categories.
    set_option(pool, "registration_default_role", json!("taxonomists")).await;
    assert_eq!(register(base, "sorter@example.com", "S", PASSWORD).0, 202);
    let (role, custom): (String, Option<String>) =
        sqlx::query_as("SELECT role, custom_role FROM users WHERE email = 'sorter@example.com'")
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!((role.as_str(), custom), ("subscriber", None));
    set_option(pool, "registration_default_role", json!("editor")).await;
    let check = registration_check(base, &cookie);
    assert_eq!(check["status"], "warn", "{check}");
    assert!(
        check["detail"].as_str().unwrap().contains("\"editor\""),
        "{check}"
    );
}

/// An administrator inviting or creating an account for an address an
/// unconfirmed sign-up holds is told so (they can see that account in the
/// list anyway), rather than "already taken".
#[tokio::test]
async fn creating_an_account_over_an_unconfirmed_sign_up_says_what_holds_the_address() {
    let db = TestDb::new().await;
    let pool = db.pool();
    open(pool).await;
    let admin = common::seed_user(pool, Role::Admin).await;
    let member = common::seed_user(pool, Role::Subscriber).await;
    unconfirmed(pool, 98_701, "held@example.com", Role::Subscriber).await;
    let server = server(&db);
    let base = server.base();
    let cookie = common::login_cookie(base, &admin.email, &admin.password);

    let create = |email: &str, password: Value| {
        outcome(http(
            ureq::post(&format!("{base}/api/v1/users"))
                .set("cookie", &cookie)
                .set("X-Forwarded-For", &next_ip())
                .send_json(json!({ "email": email, "role": "author", "password": password })),
        ))
    };
    for password in [Value::Null, json!("a-long-password-1")] {
        let (code, body) = create("Held@Example.com", password);
        assert_eq!(code, 409, "{body}");
        assert_eq!(
            body["message"],
            "an unconfirmed sign-up holds this address; confirm or delete it first",
            "{body}"
        );
    }
    // A confirmed account's address is the ordinary conflict.
    let (code, body) = create(&member.email, Value::Null);
    assert_eq!(code, 409, "{body}");
    assert!(
        body["message"].as_str().unwrap().contains("already taken"),
        "{body}"
    );
}

/// The reset form follows the one password rule every other path applies.
#[tokio::test]
async fn a_reset_applies_the_shared_password_rule() {
    let db = TestDb::new().await;
    let pool = db.pool();
    open(pool).await;
    let member = common::seed_user(pool, Role::Subscriber).await;
    let server = server(&db);
    let base = server.base();
    let raw = "vy-reset-rule-token";
    UsersRepo::new(pool.clone())
        .create_reset_token(member.id, &vyasa_core::user::registration::hash_token(raw))
        .await
        .unwrap();
    let reset = |password: &str| {
        outcome(post(
            base,
            "/auth/reset",
            &json!({ "token": raw, "password": password }),
        ))
    };
    let (code, body) = reset("short-7");
    assert_eq!(code, 400, "{body}");
    assert_eq!(
        body["message"],
        vyasa_core::user::password::validate_password("short-7")
            .unwrap_err()
            .client_message()
    );
    // Refused before the token is spent.
    assert_eq!(reset("long-enough").0, 200);
}

/// A reset link that is not live is refused before the new password is
/// checked or hashed, so a garbage token costs the server no argon2 work.
/// The order of the refusals shows it: hashing comes after the password
/// rule, and a dead link with a password the rule refuses is answered for
/// the link (404), not the password (400).
#[tokio::test]
async fn a_dead_reset_link_is_refused_before_the_password_is_looked_at() {
    let db = TestDb::new().await;
    let pool = db.pool();
    open(pool).await;
    let member = common::seed_user(pool, Role::Subscriber).await;
    let server = server(&db);
    let base = server.base();
    let reset = |token: &str, password: &str| {
        outcome(post(
            base,
            "/auth/reset",
            &json!({ "token": token, "password": password }),
        ))
    };
    let users = UsersRepo::new(pool.clone());
    let hash = vyasa_core::user::registration::hash_token;

    // Spent: used once already.
    users
        .create_reset_token(member.id, &hash("vy-spent"))
        .await
        .unwrap();
    assert_eq!(reset("vy-spent", "long-enough").0, 200);
    // Expired.
    users
        .create_reset_token(member.id, &hash("vy-expired"))
        .await
        .unwrap();
    sqlx::query(
        "UPDATE reset_tokens SET expires_at = now() - interval '1 minute' WHERE token_hash = $1",
    )
    .bind(hash("vy-expired"))
    .execute(pool)
    .await
    .unwrap();
    // Issued for confirming an address, not for setting a password.
    TokensRepo::new(pool.clone())
        .issue(
            member.id,
            TokenPurpose::Verify,
            &hash("vy-verify"),
            chrono::Duration::hours(1),
        )
        .await
        .unwrap();

    for dead in ["garbage", "vy-spent", "vy-expired", "vy-verify"] {
        let (code, body) = reset(dead, "short-7");
        assert_eq!(code, 404, "{dead}: {body}");
        let (code, body) = reset(dead, "long-enough");
        assert_eq!(code, 404, "{dead}: {body}");
    }
    // Looking did not spend the confirmation link.
    let still: bool = sqlx::query_scalar("SELECT NOT used FROM reset_tokens WHERE token_hash = $1")
        .bind(hash("vy-verify"))
        .fetch_one(pool)
        .await
        .unwrap();
    assert!(still);
    assert_eq!(login(base, &next_ip(), &member.email, "long-enough").0, 200);
}

fn registration_check(base: &str, cookie: &str) -> Value {
    let (code, report) = call(base, cookie, "GET", "/site-health");
    assert_eq!(code, 200, "{report}");
    report["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "registration")
        .cloned()
        .unwrap_or(Value::Null)
}

#[tokio::test]
async fn site_health_warns_when_registration_cannot_work_as_configured() {
    let db = TestDb::new().await;
    let pool = db.pool();
    let admin = common::seed_user(pool, Role::Admin).await;
    let server = server(&db);
    let base = server.base();
    let cookie = common::login_cookie(base, &admin.email, &admin.password);

    assert_eq!(registration_check(base, &cookie)["status"], "ok");

    set_option(pool, "registration_enabled", json!(true)).await;
    let check = registration_check(base, &cookie);
    assert_eq!(check["status"], "warn", "{check}");
    let detail = check["detail"].as_str().unwrap();
    assert!(detail.contains("mail"), "{detail}");
    assert!(detail.contains("site address"), "{detail}");

    open(pool).await;
    assert_eq!(registration_check(base, &cookie)["status"], "ok");

    // How many accounts are waiting for confirmation.
    let detail = |check: Value| check["detail"].as_str().unwrap().to_owned();
    assert!(
        detail(registration_check(base, &cookie)).contains("no account awaits confirmation"),
        "{}",
        detail(registration_check(base, &cookie))
    );
    unconfirmed(pool, 98_601, "one@example.com", Role::Subscriber).await;
    unconfirmed(pool, 98_602, "two@example.com", Role::Subscriber).await;
    let check = registration_check(base, &cookie);
    assert_eq!(check["status"], "ok", "{check}");
    assert!(
        detail(check.clone()).contains("2 accounts await confirmation"),
        "{check}"
    );

    // A default role widened past what a stranger may get is not used.
    RolesRepo::new(pool.clone())
        .insert(&NewRole {
            slug: "reader",
            name: "Reader",
            description: "",
            capabilities: &["view_admin".to_owned()],
        })
        .await
        .unwrap();
    set_option(pool, "registration_default_role", json!("reader")).await;
    assert_eq!(registration_check(base, &cookie)["status"], "ok");
    sqlx::query(
        "UPDATE roles SET capabilities = '{view_admin,manage_users}' WHERE slug = 'reader'",
    )
    .execute(pool)
    .await
    .unwrap();
    let check = registration_check(base, &cookie);
    assert_eq!(check["status"], "warn", "{check}");
    let detail = check["detail"].as_str().unwrap();
    assert!(
        detail.contains("reader") && detail.contains("subscriber"),
        "{detail}"
    );
}
