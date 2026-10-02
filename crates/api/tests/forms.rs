//! A designed form over HTTP: created, rendered by the form block, posted
//! by a visitor, validated, and read in the inbox.
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod common;

use serde_json::{json, Value};
use vyasa_testkit::{TestDb, TestServer};

const TOKEN: &str = "stp_test_token_for_forms_00001";

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

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn a_designed_form_renders_accepts_answers_and_fills_the_inbox() {
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
    http(
        ureq::post(format!("{base}/api/v1/setup/content").as_str())
            .set("cookie", &session)
            .send_json(json!({"theme": "blog"})),
    );

    // A form with a required choice; a duplicate key is refused.
    let dup = http(
        ureq::post(format!("{base}/api/v1/forms").as_str())
            .set("cookie", &session)
            .send_json(json!({"name": "Bad", "fields": [{"key": "a", "label": "A"}, {"key": "a", "label": "A again"}]})),
    );
    assert_eq!(dup.status(), 400);
    let form: Value = serde_json::from_str(&body(http(
        ureq::post(format!("{base}/api/v1/forms").as_str())
            .set("cookie", &session)
            .send_json(json!({
                "name": "Contact us",
                "fields": [
                    {"key": "name", "label": "Your name", "kind": "text", "required": true},
                    {"key": "email", "label": "Email", "kind": "email", "required": true},
                    {"key": "topic", "label": "Topic", "kind": "select", "required": true, "options": ["Sales", "Support"]},
                    {"key": "message", "label": "Message", "kind": "textarea"}
                ]
            })),
    )))
    .unwrap();
    assert_eq!(form["slug"], "contact-us");
    let fid = form["id"].as_i64().unwrap();

    // A page with the form block renders the inputs.
    let page = http(
        ureq::post(format!("{base}/api/v1/posts").as_str())
            .set("cookie", &session)
            .send_json(json!({
                "type": "page", "post_type": "page", "status": "published", "title": "Contact", "slug": "contact",
                "content": {"schema_version": 1, "blocks": [
                    {"kind": "form", "attrs": {"form_slug": "contact-us"}, "children": []}
                ]}
            })),
    );
    assert_eq!(page.status(), 201, "{}", body(page));
    let html = body(http(ureq::get(format!("{base}/contact").as_str()).call()));
    assert!(
        html.contains("name=\"topic\"") && html.contains("<option>Sales</option>"),
        "{html}"
    );
    assert!(html.contains("name=\"website\""), "honeypot present");

    // A visitor's answers: a missing required field is refused, a full
    // set is stored, a filled honeypot is silently dropped.
    let short = body(http(
        ureq::post(format!("{base}/form").as_str())
            .send_string("form=contact-us&name=Ada&email=ada%40example.com&path=%2Fcontact"),
    ));
    assert!(short.contains("Topic is required"), "{short}");
    let ok = body(http(
        ureq::post(format!("{base}/form").as_str())
            .send_string("form=contact-us&name=Ada&email=ada%40example.com&topic=Sales&message=Hi+there&path=%2Fcontact"),
    ));
    assert!(ok.contains("Thanks"), "{ok}");
    http(
        ureq::post(format!("{base}/form").as_str())
            .send_string("form=contact-us&website=spam&name=Bot&email=b%40x.y&topic=Sales"),
    );

    // The inbox has one unread answer with the data; marking it read
    // clears the count; the CSV carries the labels.
    let list: Value = serde_json::from_str(&body(http(
        ureq::get(format!("{base}/api/v1/forms").as_str())
            .set("cookie", &session)
            .call(),
    )))
    .unwrap();
    assert_eq!(list[0]["unread"], 1, "{list}");
    let inbox: Value = serde_json::from_str(&body(http(
        ureq::get(format!("{base}/api/v1/forms/{fid}/submissions").as_str())
            .set("cookie", &session)
            .call(),
    )))
    .unwrap();
    assert_eq!(inbox.as_array().unwrap().len(), 1);
    assert_eq!(inbox[0]["data"]["topic"], "Sales");
    assert_eq!(inbox[0]["email"], "ada@example.com");
    let sid = inbox[0]["id"].as_i64().unwrap();
    http(
        ureq::post(format!("{base}/api/v1/forms/submissions/read").as_str())
            .set("cookie", &session)
            .send_json(json!({"ids": [sid]})),
    );
    let list: Value = serde_json::from_str(&body(http(
        ureq::get(format!("{base}/api/v1/forms").as_str())
            .set("cookie", &session)
            .call(),
    )))
    .unwrap();
    assert_eq!(list[0]["unread"], 0);
    let csv = body(http(
        ureq::get(format!("{base}/api/v1/forms/{fid}/submissions.csv").as_str())
            .set("cookie", &session)
            .call(),
    ));
    assert!(csv.starts_with("received,page,\"Your name\""), "{csv}");
    assert!(csv.contains("\"Sales\""));
}
