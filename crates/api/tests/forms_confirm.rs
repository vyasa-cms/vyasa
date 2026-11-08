//! A form that asks for confirmation: the entry is held out of the inbox
//! until its sender clicks the emailed link, can be deleted from the same
//! mail, and nothing is mailed to the owner before the click.
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod common;

use serde_json::{json, Value};
use vyasa_testkit::{TestDb, TestServer};

const TOKEN: &str = "stp_test_token_for_forms_00002";

fn http(r: Result<ureq::Response, ureq::Error>) -> ureq::Response {
    match r {
        Ok(v) | Err(ureq::Error::Status(_, v)) => v,
        Err(ureq::Error::Transport(e)) => panic!("transport {e}"),
    }
}

fn body(r: ureq::Response) -> String {
    use std::io::Read;
    let mut s = String::new();
    r.into_reader()
        .take(5_000_000)
        .read_to_string(&mut s)
        .unwrap();
    s
}

fn cookie_of(r: &ureq::Response, name: &str) -> Option<String> {
    r.all("set-cookie")
        .iter()
        .filter_map(|c| c.split(';').next())
        .find(|kv| kv.starts_with(name))
        .map(std::string::ToString::to_string)
}

async fn count(db: &TestDb, sql: &'static str) -> i64 {
    sqlx::query_scalar(sql).fetch_one(db.pool()).await.unwrap()
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn a_confirming_form_holds_entries_until_the_link_is_clicked() {
    let db = TestDb::new().await;
    sqlx::query("INSERT INTO options (key, value) VALUES ('site_url', to_jsonb($1::text))")
        .bind("http://example.test")
        .execute(db.pool())
        .await
        .unwrap();
    let server = TestServer::builder(common::BIN, &db)
        .env("VYASA_SETUP_TOKEN", TOKEN)
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
            .send_json(json!({"email": "x@example.com", "password": "twelve-char-password"})),
    );
    let session = cookie_of(&created, "vy_session").unwrap();

    // Asking for confirmation without a required email field is refused.
    let bad = http(
        ureq::post(format!("{base}/api/v1/forms").as_str())
            .set("cookie", &session)
            .send_json(json!({"name": "No email", "confirm": true,
                "fields": [{"key": "name", "label": "Name"}]})),
    );
    assert_eq!(bad.status(), 400);

    let form: Value = serde_json::from_str(&body(http(
        ureq::post(format!("{base}/api/v1/forms").as_str())
            .set("cookie", &session)
            .send_json(json!({
                "name": "Early access", "confirm": true, "notify_email": "owner@example.com",
                "fields": [
                    {"key": "email", "label": "Email", "kind": "email", "required": true},
                    {"key": "talk", "label": "Talk to the team", "kind": "checkbox"}
                ]
            })),
    )))
    .unwrap();
    assert_eq!(form["confirm"], true);
    let fid = form["id"].as_i64().unwrap();

    // An entry held for more than a week cannot be confirmed, swept or not.
    sqlx::query(
        "INSERT INTO form_submissions (id, form, email, token, created_at)
         VALUES (1, 'early-access', 'old@example.com', 'oldtoken', now() - interval '8 days')",
    )
    .execute(db.pool())
    .await
    .unwrap();

    let sent = body(http(
        ureq::post(format!("{base}/form").as_str())
            .send_string("form=early-access&email=ada%40example.com&talk=on&path=%2Fplatform"),
    ));
    assert!(sent.contains("Check your inbox"), "{sent}");

    // Held: not in the inbox, the CSV or the unread count; no owner mail yet.
    let inbox = |base: &str, session: &str| -> Value {
        serde_json::from_str(&body(http(
            ureq::get(format!("{base}/api/v1/forms/{fid}/submissions").as_str())
                .set("cookie", session)
                .call(),
        )))
        .unwrap()
    };
    assert_eq!(inbox(base, &session).as_array().unwrap().len(), 0);
    // ...but the owner can ask for the held entries (this one and the old one).
    let held: Value = serde_json::from_str(&body(http(
        ureq::get(format!("{base}/api/v1/forms/{fid}/submissions?held=true").as_str())
            .set("cookie", &session)
            .call(),
    )))
    .unwrap();
    assert_eq!(held.as_array().unwrap().len(), 2, "{held}");
    assert_eq!(
        count(
            &db,
            "SELECT count(*) FROM jobs WHERE kind = 'send_email' AND payload->>'template' = 'form_submission'"
        )
        .await,
        0
    );

    let stale = body(http(
        ureq::post(format!("{base}/form/confirm").as_str()).send_string("token=oldtoken"),
    ));
    assert!(stale.contains("expired"), "{stale}");
    assert_eq!(
        count(
            &db,
            "SELECT count(*) FROM form_submissions WHERE token = 'oldtoken' AND confirmed_at IS NOT NULL"
        )
        .await,
        0
    );
    let token: String =
        sqlx::query_scalar("SELECT token FROM form_submissions WHERE email = 'ada@example.com'")
            .fetch_one(db.pool())
            .await
            .unwrap();

    // The confirmation mail was queued to the sender with both links.
    let mail: Value = sqlx::query_scalar(
        "SELECT payload FROM jobs WHERE kind = 'send_email' AND payload->>'template' = 'form_confirm'",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(mail["to"], "ada@example.com");
    let text = mail["body"].as_str().unwrap();
    assert!(
        text.contains(&format!("http://example.test/form/confirm?token={token}"))
            && text.contains(&format!("http://example.test/form/delete?token={token}")),
        "{text}"
    );

    // A mail scanner that opens both links changes nothing.
    for path in ["confirm", "delete"] {
        let page = body(http(
            ureq::get(format!("{base}/form/{path}?token={token}").as_str()).call(),
        ));
        assert!(page.contains("<button"), "{page}");
    }
    assert_eq!(inbox(base, &session).as_array().unwrap().len(), 0);
    assert_eq!(
        count(
            &db,
            "SELECT count(*) FROM form_submissions WHERE token = (SELECT token FROM form_submissions WHERE email = 'ada@example.com') AND confirmed_at IS NULL"
        )
        .await,
        1
    );

    let ok = body(http(
        ureq::post(format!("{base}/form/confirm").as_str()).send_string(&format!("token={token}")),
    ));
    assert!(ok.contains("confirmed"), "{ok}");
    let rows = inbox(base, &session);
    assert_eq!(rows.as_array().unwrap().len(), 1);
    assert_eq!(rows[0]["data"]["talk"], "on");
    let csv = body(http(
        ureq::get(format!("{base}/api/v1/forms/{fid}/submissions.csv").as_str())
            .set("cookie", &session)
            .call(),
    ));
    assert!(csv.contains("ada@example.com"), "{csv}");

    // A second click changes nothing; the owner is mailed once, and that
    // mail carries no answers and no address.
    let again = body(http(
        ureq::post(format!("{base}/form/confirm").as_str()).send_string(&format!("token={token}")),
    ));
    assert!(again.contains("already used"), "{again}");
    assert_eq!(
        count(
            &db,
            "SELECT count(*) FROM jobs WHERE kind = 'send_email' AND payload->>'template' = 'form_confirmed_notice'"
        )
        .await,
        1
    );
    assert_eq!(
        count(
            &db,
            "SELECT count(*) FROM jobs WHERE payload->>'template' = 'form_submission'"
        )
        .await,
        0
    );
    let notice: Value = sqlx::query_scalar(
        "SELECT payload FROM jobs WHERE payload->>'template' = 'form_confirmed_notice'",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    let text = notice["body"].as_str().unwrap();
    assert!(!text.contains("ada@example.com"));
    assert!(text.contains("now has 1 confirmed entry"), "{text}");
    assert!(text.contains("/admin/forms"), "{text}");

    // A repeat submission replaces its held entry; an invalid address and an
    // empty one are refused and leave no row.
    for addr in ["bob%40example.com", "BOB%40Example.com"] {
        http(
            ureq::post(format!("{base}/form").as_str())
                .send_string(&format!("form=early-access&email={addr}&path=%2Fplatform")),
        );
    }
    assert_eq!(
        count(
            &db,
            "SELECT count(*) FROM form_submissions WHERE lower(email) = 'bob@example.com'"
        )
        .await,
        1
    );
    // (The form endpoint allows five posts a minute per address, so the
    // invalid-address cases share one request.)
    let short = body(http(
        ureq::post(format!("{base}/form").as_str())
            .send_string("form=early-access&email=a%40b&path=%2Fplatform"),
    ));
    assert!(short.contains("must be an email address") || short.contains("doesn't look right"));
    assert_eq!(
        count(
            &db,
            "SELECT count(*) FROM form_submissions WHERE email = 'a@b'"
        )
        .await,
        0
    );

    // A held entry can be deleted by its own link before it is confirmed.
    let bob_token: String = sqlx::query_scalar(
        "SELECT token FROM form_submissions WHERE lower(email) = 'bob@example.com'",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    let held_gone = body(http(
        ureq::post(format!("{base}/form/delete").as_str())
            .send_string(&format!("token={bob_token}")),
    ));
    assert!(held_gone.contains("deleted"), "{held_gone}");
    assert_eq!(
        count(
            &db,
            "SELECT count(*) FROM form_submissions WHERE lower(email) = 'bob@example.com'"
        )
        .await,
        0
    );

    // The delete link removes a confirmed entry.
    let gone = body(http(
        ureq::post(format!("{base}/form/delete").as_str()).send_string(&format!("token={token}")),
    ));
    assert!(gone.contains("deleted"), "{gone}");
    assert_eq!(inbox(base, &session).as_array().unwrap().len(), 0);

    // An optional email field before the required one does not decide where
    // the confirmation goes, and cannot be used to skip it.
    let two: Value = serde_json::from_str(&body(http(
        ureq::post(format!("{base}/api/v1/forms").as_str())
            .set("cookie", &session)
            .send_json(json!({
                "name": "Two emails", "confirm": true,
                "fields": [
                    {"key": "cc", "label": "CC", "kind": "email"},
                    {"key": "email", "label": "Email", "kind": "email", "required": true}
                ]
            })),
    )))
    .unwrap();
    assert_eq!(two["confirm"], true);
    http(
        ureq::post(format!("{base}/form").as_str())
            .send_string("form=two-emails&cc=spy%40example.com&email=eve%40example.com&path=%2Fx"),
    );
    let to: String = sqlx::query_scalar(
        "SELECT payload->>'to' FROM jobs WHERE payload->>'template' = 'form_confirm' ORDER BY id DESC LIMIT 1",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(to, "eve@example.com");
    let list: Value = serde_json::from_str(&body(http(
        ureq::get(format!("{base}/api/v1/forms").as_str())
            .set("cookie", &session)
            .call(),
    )))
    .unwrap();
    assert!(
        list.as_array().unwrap().iter().all(|f| f["unread"] == 0),
        "held entries are not counted unread: {list}"
    );
}

#[tokio::test]
async fn a_consent_field_must_be_given_and_is_stored_with_its_time() {
    let db = TestDb::new().await;
    let server = TestServer::builder(common::BIN, &db)
        .env("VYASA_SETUP_TOKEN", TOKEN)
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
            .send_json(json!({"email": "x@example.com", "password": "twelve-char-password"})),
    );
    let session = cookie_of(&created, "vy_session").unwrap();

    // A consent field that is not required, or links somewhere odd, is refused.
    for field in [
        json!({"key": "ok", "label": "I agree", "kind": "consent"}),
        json!({"key": "ok", "label": "I agree", "kind": "consent", "required": true,
               "options": ["javascript:alert(1)"]}),
    ] {
        let bad = http(
            ureq::post(format!("{base}/api/v1/forms").as_str())
                .set("cookie", &session)
                .send_json(json!({"name": "Bad", "fields": [field]})),
        );
        assert_eq!(bad.status(), 400);
    }
    let made = http(
        ureq::post(format!("{base}/api/v1/forms").as_str())
            .set("cookie", &session)
            .send_json(json!({"name": "Signup", "fields": [
                {"key": "email", "label": "Email", "kind": "email", "required": true},
                {"key": "ok", "label": "I agree", "kind": "consent", "required": true,
                 "options": ["/privacy"]}
            ]})),
    );
    assert_eq!(made.status(), 201);

    // Without the box ticked the entry is refused; with it, stored with the time.
    let refused = body(http(
        ureq::post(format!("{base}/form").as_str())
            .send_string("form=signup&email=a%40example.com&path=%2F"),
    ));
    assert!(refused.contains("is required"), "{refused}");
    assert_eq!(count(&db, "SELECT count(*) FROM form_submissions").await, 0);
    http(
        ureq::post(format!("{base}/form").as_str())
            .send_string("form=signup&email=a%40example.com&ok=yes&path=%2F"),
    );
    let data: Value = sqlx::query_scalar("SELECT data FROM form_submissions")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(data["ok"], "yes");
    assert!(
        data["ok_consent_at"].as_str().unwrap().contains('T'),
        "{data}"
    );
}
