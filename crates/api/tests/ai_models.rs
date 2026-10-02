//! The model registry over REST: keys never echo, defaults follow the
//! rules, and nothing here needs a real provider.
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod common;

use serde_json::{json, Value};
use sqlx::PgPool;
use vyasa_db::{models::Role, repo::NewUser, repo::UsersRepo};
use vyasa_testkit::{TestDb, TestServer};

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

fn login(base: &str, email: &str) -> String {
    let resp = http(
        ureq::post(format!("{base}/api/v1/auth/login").as_str())
            .send_json(json!({"email": email, "password": "pw-secret-1"})),
    );
    assert_eq!(resp.status(), 200, "login {email}");
    let token = resp
        .all("set-cookie")
        .join("; ")
        .split(';')
        .next()
        .and_then(|kv| kv.split('=').nth(1))
        .unwrap()
        .to_string();
    format!("vy_session={token}")
}

async fn seed(pool: &PgPool) {
    let hash = vyasa_core::user::password::hash_password("pw-secret-1").unwrap();
    let users = UsersRepo::new(pool.clone());
    users
        .insert(&NewUser {
            id: 83_001,
            email: "models-admin@example.com",
            username: "modelsadmin",
            display_name: "Models Admin",
            password_hash: Some(&hash),
            role: Role::Admin,
            bio: "",
        })
        .await
        .expect("admin");
    users
        .insert(&NewUser {
            id: 83_002,
            email: "models-author@example.com",
            username: "modelsauthor",
            display_name: "Models Author",
            password_hash: Some(&hash),
            role: Role::Author,
            bio: "",
        })
        .await
        .expect("author");
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn registry_keeps_keys_secret_and_defaults_honest() {
    let db = TestDb::new().await;
    seed(db.pool()).await;
    let server = TestServer::builder(common::BIN, &db)
        .env("VYASA_SECRET_KEY", "test-secret-for-sealing-keys")
        .start();
    let base = server.base();
    let admin = login(base, "models-admin@example.com");
    let author = login(base, "models-author@example.com");

    // Site configuration: authors are kept out.
    let denied = http(
        ureq::get(format!("{base}/api/v1/ai/models").as_str())
            .set("cookie", &author)
            .call(),
    );
    assert_eq!(denied.status(), 403);

    // Empty registry: three known providers, none configured, key encryption on.
    let overview = json_body(http(
        ureq::get(format!("{base}/api/v1/ai/models").as_str())
            .set("cookie", &admin)
            .call(),
    ));
    assert_eq!(overview["encrypting_keys"], true);
    assert_eq!(overview["providers"].as_array().unwrap().len(), 6);
    assert!(overview["providers"]
        .as_array()
        .unwrap()
        .iter()
        .all(|p| p["configured"] == false));
    assert_eq!(overview["models"].as_array().unwrap().len(), 0);
    assert_eq!(overview["kinds"].as_array().unwrap().len(), 7);

    // Store a key: the response shows a hint, never the key.
    let saved = json_body(http(
        ureq::put(format!("{base}/api/v1/ai/providers/openai").as_str())
            .set("cookie", &admin)
            .send_json(json!({"api_key": "sk-test-abcdef-9876", "base_url": ""})),
    ));
    assert_eq!(saved["configured"], true);
    assert_eq!(saved["key_source"], "stored");
    assert_eq!(saved["key_sealed"], true);
    assert_eq!(saved["key_hint"], "…9876");
    assert!(
        !saved.to_string().contains("abcdef"),
        "the key leaked: {saved}"
    );

    // Anthropic cannot serve embeddings; the registry says no.
    let wrong = http(
        ureq::post(format!("{base}/api/v1/ai/models").as_str())
            .set("cookie", &admin)
            .send_json(json!({"provider": "anthropic", "kind": "embedding", "model": "x"})),
    );
    assert_eq!(wrong.status(), 400);

    // First text model becomes the default; the second does not.
    let first = json_body(http(
        ureq::post(format!("{base}/api/v1/ai/models").as_str())
            .set("cookie", &admin)
            .send_json(json!({"provider": "openai", "kind": "text", "model": "gpt-4o-mini", "label": "GPT-4o mini"})),
    ));
    assert_eq!(first["is_default"], true);
    let first_id = first["id"].as_i64().unwrap();
    let second = json_body(http(
        ureq::post(format!("{base}/api/v1/ai/models").as_str())
            .set("cookie", &admin)
            .send_json(json!({"provider": "openrouter", "kind": "text", "model": "anthropic/claude-sonnet-4"})),
    ));
    assert_eq!(second["is_default"], false);
    let second_id = second["id"].as_i64().unwrap();

    // Registering the same model twice for the same kind is a conflict.
    let dup = http(
        ureq::post(format!("{base}/api/v1/ai/models").as_str())
            .set("cookie", &admin)
            .send_json(json!({"provider": "openai", "kind": "text", "model": "gpt-4o-mini"})),
    );
    assert_eq!(dup.status(), 409);

    // Promote the second: exactly one default per kind.
    let promoted = json_body(http(
        ureq::post(format!("{base}/api/v1/ai/models/{second_id}/default").as_str())
            .set("cookie", &admin)
            .call(),
    ));
    assert_eq!(promoted["is_default"], true);
    let models = json_body(http(
        ureq::get(format!("{base}/api/v1/ai/models").as_str())
            .set("cookie", &admin)
            .call(),
    ))["models"]
        .clone();
    let defaults: Vec<&Value> = models
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["is_default"] == true)
        .collect();
    assert_eq!(defaults.len(), 1);
    assert_eq!(defaults[0]["id"].as_i64().unwrap(), second_id);

    // Editing: costs can be set and cleared.
    let edited = json_body(http(
        ureq::put(format!("{base}/api/v1/ai/models/{first_id}").as_str())
            .set("cookie", &admin)
            .send_json(
                json!({"label": "Mini", "input_cost_per_mtok": 0.15, "output_cost_per_mtok": 0.6}),
            ),
    ));
    assert_eq!(edited["label"], "Mini");
    assert_eq!(edited["input_cost_per_mtok"], 0.15);
    let cleared = json_body(http(
        ureq::put(format!("{base}/api/v1/ai/models/{first_id}").as_str())
            .set("cookie", &admin)
            .send_json(json!({"input_cost_per_mtok": null})),
    ));
    assert!(cleared["input_cost_per_mtok"].is_null());
    assert_eq!(
        cleared["output_cost_per_mtok"], 0.6,
        "an absent field must not clear"
    );

    // Deleting the default hands it to the other enabled model.
    let gone = http(
        ureq::delete(format!("{base}/api/v1/ai/models/{second_id}").as_str())
            .set("cookie", &admin)
            .call(),
    );
    assert_eq!(gone.status(), 204);
    let remaining = json_body(http(
        ureq::get(format!("{base}/api/v1/ai/models").as_str())
            .set("cookie", &admin)
            .call(),
    ))["models"]
        .clone();
    assert_eq!(remaining.as_array().unwrap().len(), 1);
    assert_eq!(remaining[0]["is_default"], true);

    // A provider with no key anywhere cannot be tested or browsed, and
    // says what to do.
    let no_key = http(
        ureq::post(format!("{base}/api/v1/ai/providers/anthropic/test").as_str())
            .set("cookie", &admin)
            .call(),
    );
    assert_eq!(no_key.status(), 400);
    assert!(json_body(no_key)["message"]
        .as_str()
        .unwrap()
        .contains("Models page"));

    // Clearing a key falls back to "none".
    let cleared_key = json_body(http(
        ureq::put(format!("{base}/api/v1/ai/providers/openai").as_str())
            .set("cookie", &admin)
            .send_json(json!({"api_key": ""})),
    ));
    assert_eq!(cleared_key["configured"], false);
    assert_eq!(cleared_key["key_source"], "none");

    // Removing a provider removes its models.
    let removed = http(
        ureq::delete(format!("{base}/api/v1/ai/providers/openai").as_str())
            .set("cookie", &admin)
            .call(),
    );
    assert_eq!(removed.status(), 204);
    let after = json_body(http(
        ureq::get(format!("{base}/api/v1/ai/models").as_str())
            .set("cookie", &admin)
            .call(),
    ));
    assert_eq!(after["models"].as_array().unwrap().len(), 0);
}
