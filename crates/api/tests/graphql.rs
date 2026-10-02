//! GraphQL query integration tests — batt + authz via live server.
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::items_after_statements,
    clippy::too_many_lines
)]
#![allow(unused)]
#![allow(clippy::all, clippy::pedantic, clippy::nursery)]

mod common;

use serde_json::{json, Value};
use sqlx::PgPool;
use vyasa_db::models::Role;
use vyasa_db::repo::{NewTerm, NewUser, TermsRepo, UsersRepo};
use vyasa_testkit::{TestDb, TestServer};

fn http(r: Result<ureq::Response, ureq::Error>) -> ureq::Response {
    match r {
        Ok(v) | Err(ureq::Error::Status(_, v)) => v,
        Err(ureq::Error::Transport(e)) => panic!("transport {e}"),
    }
}
fn json_body(r: ureq::Response) -> Value {
    let mut s = String::new();
    use std::io::Read;
    r.into_reader()
        .take(2_000_000)
        .read_to_string(&mut s)
        .unwrap();
    serde_json::from_str(&s).unwrap_or_else(|e| panic!("parse json {e}: {s}"))
}
fn login_token(base: &str, email: &str) -> String {
    let resp = http(
        ureq::post(format!("{base}/api/v1/auth/login").as_str())
            .send_json(json!({"email": email, "password": "pw-secret-1"})),
    );
    assert_eq!(resp.status(), 200, "login {email}");
    resp.all("set-cookie")
        .join("; ")
        .split(';')
        .next()
        .and_then(|kv| kv.split('=').nth(1))
        .unwrap()
        .to_string()
}

async fn seed_users_and_terms(pool: &PgPool) {
    let users = UsersRepo::new(pool.clone());
    let hash = vyasa_core::user::password::hash_password("pw-secret-1").unwrap();
    for (id, email, uname, role) in [
        (
            91_101,
            "admin-gql-http@example.com",
            "admingqlhttp",
            Role::Admin,
        ),
        (
            91_102,
            "author-gql-http@example.com",
            "authorgqlhttp",
            Role::Author,
        ),
        (
            91_103,
            "sub-gql-http@example.com",
            "subgqlhttp",
            Role::Subscriber,
        ),
    ] {
        users
            .insert(&NewUser {
                id,
                email,
                username: uname,
                display_name: uname,
                password_hash: Some(&hash),
                role,
                bio: "",
            })
            .await
            .expect("user");
    }
    let terms = TermsRepo::new(pool.clone());
    terms
        .insert(&NewTerm {
            id: 91_110,
            taxonomy: vyasa_db::content_models::Taxonomy::Category,
            name: "TechHttp",
            slug: "tech-http",
            parent_id: None,
            meta: serde_json::json!({}),
        })
        .await
        .expect("term");
    let options = vyasa_db::repo::OptionsRepo::new(pool.clone());
    options
        .set("site_title", &json!("Vyasa GQL Http"))
        .await
        .expect("option");
    let api_keys = vyasa_db::repo::ApiKeysRepo::new(pool.clone());
    let raw = "vy_testkey123";
    let hash_hex = {
        use sha2::{Digest, Sha256};
        let d = Sha256::digest(raw.as_bytes());
        hex::encode(d)
    };
    api_keys
        .insert(
            91_120,
            91_103,
            "test key",
            &hash_hex,
            &json!(["view_admin"]),
        )
        .await
        .expect("api key");
}

#[tokio::test]
async fn graphql_query_auth_and_published() {
    let db = TestDb::new().await;
    seed_users_and_terms(db.pool()).await;
    let server = TestServer::start(common::BIN, &db);
    let base = server.base();

    let admin_cookie = login_token(base, "admin-gql-http@example.com");
    let author_cookie = login_token(base, "author-gql-http@example.com");
    let sub_cookie = login_token(base, "sub-gql-http@example.com");

    // Create 3 published posts with term via REST as admin, and one draft as author
    let valid_content =
        json!({"schema_version":1,"blocks":[{"kind":"paragraph","attrs":{"text":"hi"}}]});
    for i in 0..3 {
        let resp = http(
            ureq::post(format!("{base}/api/v1/posts").as_str())
                .set("cookie", &format!("vy_session={admin_cookie}"))
                .send_json(json!({
                    "title": format!("GQL Http Post {}", i + 1),
                    "content": valid_content.clone(),
                    "status": "published",
                    "term_ids": [91_110]
                })),
        );
        assert_eq!(resp.status(), 201, "create post {}", i + 1);
    }
    // draft by author (no term_ids, status default draft)
    let draft_resp = http(
        ureq::post(format!("{base}/api/v1/posts").as_str())
            .set("cookie", &format!("vy_session={author_cookie}"))
            .send_json(json!({
                "title": "Draft Http Secret",
                "content": valid_content.clone()
            })),
    );
    assert_eq!(draft_resp.status(), 201);
    // ensure draft is indeed draft (title check)
    let draft_body = json_body(draft_resp);
    assert_eq!(draft_body["status"], "draft");
    let created_draft_id = draft_body["id"].as_i64().unwrap();

    let gql = |query: &str, cookie: Option<&str>, bearer: Option<&str>| -> Value {
        let mut req = ureq::post(format!("{base}/api/graphql").as_str());
        if let Some(c) = cookie {
            req = req.set("cookie", &format!("vy_session={c}"));
        }
        if let Some(b) = bearer {
            req = req.set("authorization", &format!("Bearer {b}"));
        }
        let resp = http(req.send_json(json!({"query": query})));
        assert_eq!(resp.status(), 200, "graphql http status for {query}");
        json_body(resp)
    };

    // 1. Anonymous posts only published (3)
    let anon_posts = gql(
        r#"{ posts(first: 10) { edges { node { id title status author { username } terms { name } } } totalCount } }"#,
        None,
        None,
    );
    assert!(
        anon_posts["errors"].is_null(),
        "anon should not error: {:?}",
        anon_posts["errors"]
    );
    let anon_edges = anon_posts["data"]["posts"]["edges"].as_array().unwrap();
    assert_eq!(
        anon_edges.len(),
        3,
        "anon sees 3 published, got {:?}",
        anon_posts
    );
    for e in anon_edges {
        assert_eq!(e["node"]["status"], "published");
        assert_eq!(e["node"]["author"]["username"], "admingqlhttp");
        assert_eq!(e["node"]["terms"][0]["name"], "TechHttp");
    }

    // 2. Admin sees draft + published (4)
    let admin_posts = gql(
        r#"{ posts(first: 10) { edges { node { status } } } }"#,
        Some(&admin_cookie),
        None,
    );
    assert_eq!(
        admin_posts["data"]["posts"]["edges"]
            .as_array()
            .unwrap()
            .len(),
        4
    );

    // 3. Author sees own draft + published (4)
    let author_posts = gql(
        r#"{ posts(first: 10) { edges { node { status } } } }"#,
        Some(&author_cookie),
        None,
    );
    assert_eq!(
        author_posts["data"]["posts"]["edges"]
            .as_array()
            .unwrap()
            .len(),
        4
    );

    // 4. Subscriber sees only published (3)
    let sub_posts = gql(
        r#"{ posts(first: 10) { edges { node { status } } } }"#,
        Some(&sub_cookie),
        None,
    );
    assert_eq!(
        sub_posts["data"]["posts"]["edges"]
            .as_array()
            .unwrap()
            .len(),
        3
    );

    // 5. API key bearer should see published (like anon but authenticated)
    let bearer_posts = gql(
        r#"{ posts(first: 10) { edges { node { status } } } }"#,
        None,
        Some("vy_testkey123"),
    );
    assert!(bearer_posts["errors"].is_null());
    assert_eq!(
        bearer_posts["data"]["posts"]["edges"]
            .as_array()
            .unwrap()
            .len(),
        3
    );

    // 6. me without auth -> error
    let me_anon = gql(r#"{ me { id username } }"#, None, None);
    assert!(!me_anon["errors"].is_null(), "me anon should error");

    // 7. me with admin -> ok
    let me_admin = gql(r#"{ me { id username } }"#, Some(&admin_cookie), None);
    assert!(me_admin["errors"].is_null());
    assert_eq!(me_admin["data"]["me"]["username"], "admingqlhttp");

    // 8. users requires manage_users — subscriber should error, admin ok
    let users_sub = gql(r#"{ users(first: 2) { id } }"#, Some(&sub_cookie), None);
    assert!(!users_sub["errors"].is_null(), "sub users should error");
    let users_admin = gql(r#"{ users(first: 2) { id } }"#, Some(&admin_cookie), None);
    assert!(users_admin["errors"].is_null());
    assert!(users_admin["data"]["users"].is_array());

    // 9. option public readable anonymously
    let opt = gql(r#"{ option(key: "site_title") }"#, None, None);
    assert!(opt["errors"].is_null());
    assert_eq!(opt["data"]["option"], json!("Vyasa GQL Http"));

    // 10. terms query
    let terms = gql(
        r#"{ terms(taxonomy: "category") { term { name } postCount } }"#,
        None,
        None,
    );
    assert!(terms["errors"].is_null());
    assert_eq!(terms["data"]["terms"][0]["term"]["name"], "TechHttp");

    // 11. post by id authz — draft hidden from anon
    let draft_id = created_draft_id;
    let anon_fetch = http(
        ureq::post(format!("{base}/api/graphql").as_str())
            .send_json(json!({"query": format!("{{ post(id: {draft_id}) {{ id status }} }}")})),
    );
    let anon_fetch_body = json_body(anon_fetch);
    assert!(
        !anon_fetch_body["errors"].is_null(),
        "anon fetching draft should be forbidden"
    );
    let draft_author = gql(
        &format!("{{ post(id: {draft_id}) {{ id status }} }}"),
        Some(&author_cookie),
        None,
    );
    assert!(draft_author["errors"].is_null());
    assert_eq!(draft_author["data"]["post"]["status"], "draft");

    // 12. comments (no comments yet, should be empty)
    let some_published_id = anon_edges[0]["node"]["id"].as_i64().unwrap();
    let comments = gql(
        &format!(
            "{{ comments(postId: {some_published_id}, first: 2) {{ edges {{ node {{ id }} }} totalCount }} }}"
        ),
        None,
        None,
    );
    assert!(comments["errors"].is_null());
    assert_eq!(comments["data"]["comments"]["totalCount"], 0);

    // 13. cursor pagination — first 2, then after
    let first_two = gql(
        r#"{ posts(first: 2) { edges { cursor node { title } } pageInfo { hasNextPage endCursor } totalCount } }"#,
        None,
        None,
    );
    assert!(first_two["errors"].is_null());
    let edges2 = first_two["data"]["posts"]["edges"].as_array().unwrap();
    assert_eq!(edges2.len(), 2);
    assert_eq!(
        first_two["data"]["posts"]["pageInfo"]["hasNextPage"],
        json!(true)
    );
    let end = first_two["data"]["posts"]["pageInfo"]["endCursor"]
        .as_str()
        .unwrap()
        .to_string();
    let next_page = gql(
        &format!(
            r#"{{ posts(first: 2, after: "{end}") {{ edges {{ node {{ title }} }} pageInfo {{ hasNextPage }} }} }}"#
        ),
        None,
        None,
    );
    assert!(next_page["errors"].is_null());
    let edges_next = next_page["data"]["posts"]["edges"].as_array().unwrap();
    assert_eq!(edges_next.len(), 1, "remaining 1");
    assert_eq!(
        next_page["data"]["posts"]["pageInfo"]["hasNextPage"],
        json!(false)
    );

    // A GET may read but never write: the session cookie rides along on a
    // cross-site top-level GET, and CSRF checks cover unsafe methods only.
    let get_mutation = http(
        ureq::get(format!("{base}/api/graphql").as_str())
            .query(
                "query",
                &format!("mutation {{ deletePost(id: {draft_id}) {{ id }} }}"),
            )
            .set("cookie", &format!("vy_session={admin_cookie}"))
            .call(),
    );
    assert_eq!(get_mutation.status(), 405, "mutation over GET refused");
    let still_there = gql(
        &format!("{{ post(id: {draft_id}) {{ id }} }}"),
        Some(&admin_cookie),
        None,
    );
    assert!(
        still_there["errors"].is_null() && !still_there["data"]["post"].is_null(),
        "the draft survives: {still_there}"
    );
    let get_query = http(
        ureq::get(format!("{base}/api/graphql").as_str())
            .query("query", "{ posts(first: 1) { totalCount } }")
            .call(),
    );
    assert_eq!(
        get_query.status(),
        200,
        "a plain query over GET still works"
    );
}
