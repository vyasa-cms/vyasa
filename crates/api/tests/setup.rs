//! First-run setup over HTTP: the token gate, the account step signing
//! you in, the later steps, and the door closing behind you.
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod common;

use serde_json::{json, Value};
use vyasa_testkit::{TestDb, TestServer};

const TOKEN: &str = "stp_test_token_for_setup_0001";

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
    serde_json::from_str(&s).unwrap()
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
async fn the_wizard_runs_once_and_then_closes() {
    // First-boot behaviour on an empty database: the clone from the
    // migrated template already has no rows, so no seeding here.
    let db = TestDb::new().await;
    let server = TestServer::builder(common::BIN, &db)
        .env("VYASA_SETUP_TOKEN", TOKEN)
        .start();
    let base = server.base();

    // Fresh: needs an admin, and the steps are locked without the token.
    let status = json_body(http(
        ureq::get(format!("{base}/api/v1/setup/status").as_str()).call(),
    ));
    assert_eq!(status["needs_admin"], true);
    assert_eq!(status["needs_setup"], true);
    let locked = http(ureq::get(format!("{base}/api/v1/setup/checks").as_str()).call());
    assert_eq!(locked.status(), 401);
    let wrong = http(
        ureq::post(format!("{base}/api/v1/setup/claim").as_str())
            .send_json(json!({"token": "nope"})),
    );
    assert_eq!(wrong.status(), 403);

    // The right token opens the door.
    let claimed = http(
        ureq::post(format!("{base}/api/v1/setup/claim").as_str())
            .send_json(json!({"token": TOKEN})),
    );
    assert_eq!(claimed.status(), 204);
    let setup_cookie = cookie_of(&claimed, "vy_setup").expect("setup cookie");
    let checks = json_body(http(
        ureq::get(format!("{base}/api/v1/setup/checks").as_str())
            .set("cookie", &setup_cookie)
            .call(),
    ));
    assert!(
        checks
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["name"] == "database" && c["status"] == "ok"),
        "{checks}"
    );

    // The account step creates the admin and signs them in.
    let created = http(
        ureq::post(format!("{base}/api/v1/setup/account").as_str())
            .set("cookie", &setup_cookie)
            .send_json(json!({"email": "first@example.com", "username": "first", "display_name": "First", "password": "twelve-char-password", "timezone": "Europe/Stockholm"})),
    );
    assert_eq!(created.status(), 201);
    let session = cookie_of(&created, "vy_session").expect("session cookie");

    // The setup cookie no longer works: an administrator exists.
    let stale = http(
        ureq::get(format!("{base}/api/v1/setup/checks").as_str())
            .set("cookie", &setup_cookie)
            .call(),
    );
    assert_eq!(stale.status(), 401);
    let again = http(
        ureq::post(format!("{base}/api/v1/setup/claim").as_str())
            .send_json(json!({"token": TOKEN})),
    );
    assert_eq!(again.status(), 410);

    // The rest run as the admin and land in the options.
    let site = json_body(http(
        ureq::post(format!("{base}/api/v1/setup/site").as_str())
            .set("cookie", &session)
            .send_json(json!({"site_title": "Probe", "site_tagline": "t", "site_url": base, "site_language": "en"})),
    ));
    // The server only fetches public addresses, so a loopback site address
    // is reported as local and untested rather than as unreachable.
    assert_eq!(site["site_url_verified"], false, "{site}");
    assert_eq!(site["site_url_local"], true, "{site}");
    for (url, local) in [
        (base, true),
        ("http://localhost:1", true),
        ("http://169.254.169.254/latest", true),
        ("http://[::1]:1", true),
        ("http://10.1.2.3", true),
        ("http://name.invalid", false),
    ] {
        let checked = json_body(http(
            ureq::get(format!("{base}/api/v1/setup/verify-url").as_str())
                .query("url", url)
                .set("cookie", &session)
                .call(),
        ));
        assert_eq!(checked["reachable"], false, "{url}: {checked}");
        assert_eq!(checked["local"], local, "{url}: {checked}");
    }
    let content = http(
        ureq::post(format!("{base}/api/v1/setup/content").as_str())
            .set("cookie", &session)
            .send_json(json!({"theme": "docs", "sample_content": true, "posts_per_page": 7})),
    );
    assert_eq!(content.status(), 204);
    let posts = json_body(http(
        ureq::get(format!("{base}/api/v1/posts?per_page=10").as_str())
            .set("cookie", &session)
            .call(),
    ));
    let titles: Vec<String> = posts["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["title"].as_str().unwrap().to_owned())
        .collect();
    assert!(
        titles.iter().any(|t| t.starts_with("Welcome")) && titles.iter().any(|t| t == "About"),
        "{titles:?}"
    );
    let themes = json_body(http(
        ureq::get(format!("{base}/api/v1/themes").as_str())
            .set("cookie", &session)
            .call(),
    ));
    assert!(
        themes
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["name"] == "docs" && t["is_active"] == true),
        "{themes}"
    );
    let status = json_body(http(
        ureq::get(format!("{base}/api/v1/setup/status").as_str()).call(),
    ));
    assert_eq!(status["step"], "delivery");

    for (step, body) in [
        ("delivery", json!({"edge_cache_seconds": 30})),
        (
            "mail",
            json!({"comment_moderation": "require_all", "newsletter_enabled": true}),
        ),
        (
            "assistants",
            json!({"monthly_cap_usd": 5, "alt_text": true}),
        ),
        (
            "updates",
            json!({"registry_url": "https://marketplace.example.com/index.json"}),
        ),
    ] {
        let r = http(
            ureq::post(format!("{base}/api/v1/setup/{step}").as_str())
                .set("cookie", &session)
                .send_json(body),
        );
        let code = r.status();
        assert_eq!(code, 204, "{step}: {}", r.into_string().unwrap_or_default());
    }
    let opts = json_body(http(
        ureq::get(format!("{base}/api/v1/options").as_str())
            .set("cookie", &session)
            .call(),
    ));
    assert_eq!(opts["site_title"], "Probe");
    assert_eq!(opts["posts_per_page"], 7);
    assert_eq!(opts["edge_cache_seconds"], 30);
    // No relay in this test, so the newsletter stays off however it was asked.
    assert_eq!(opts["newsletter_enabled"], false);

    // The mail relay from the admin panel: saved with a sealed password,
    // read back without it, and cleared by an empty host.
    let saved = json_body(http(
        ureq::put(format!("{base}/api/v1/mail/settings").as_str())
            .set("cookie", &session)
            .send_json(json!({"host": "smtp.example.com", "port": 587, "username": "apikey", "password": "hunter22", "from": "hello@example.com"})),
    ));
    assert_eq!(saved["source"], "options");
    assert_eq!(saved["has_password"], true);
    assert!(
        saved.get("password").is_none(),
        "the password never comes back"
    );
    let opts = json_body(http(
        ureq::get(format!("{base}/api/v1/options").as_str())
            .set("cookie", &session)
            .call(),
    ));
    assert_ne!(
        opts["smtp_password"], "hunter22",
        "stored sealed, not plain"
    );
    let checks = json_body(http(
        ureq::get(format!("{base}/api/v1/setup/checks").as_str())
            .set("cookie", &session)
            .call(),
    ));
    let smtp = checks
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "smtp")
        .unwrap();
    assert_eq!(smtp["status"], "ok", "{smtp}");
    let cleared = json_body(http(
        ureq::put(format!("{base}/api/v1/mail/settings").as_str())
            .set("cookie", &session)
            .send_json(json!({"host": "", "from": ""})),
    ));
    assert_eq!(cleared["source"], "none");

    let finished = json_body(http(
        ureq::post(format!("{base}/api/v1/setup/finish").as_str())
            .set("cookie", &session)
            .call(),
    ));
    assert!(finished["indexed"].as_u64().unwrap() >= 2, "{finished}");
    let status = json_body(http(
        ureq::get(format!("{base}/api/v1/setup/status").as_str()).call(),
    ));
    assert_eq!(status["needs_setup"], false);
    assert_eq!(status["step"], "done");
}
