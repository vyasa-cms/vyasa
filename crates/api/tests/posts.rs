//! Posts CRUD tests against live server.
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::items_after_statements
)]

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
    let mut s = String::new();
    use std::io::Read;
    r.into_reader()
        .take(1_000_000)
        .read_to_string(&mut s)
        .unwrap();
    serde_json::from_str(&s).unwrap()
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
async fn seed(pool: &PgPool) {
    let users = UsersRepo::new(pool.clone());
    let hash = vyasa_core::user::password::hash_password("pw-secret-1").unwrap();
    users
        .insert(&NewUser {
            id: 81_001,
            email: "admin-post@example.com",
            username: "adminpost",
            display_name: "AdminPost",
            password_hash: Some(&hash),
            role: Role::Admin,
            bio: "",
        })
        .await
        .expect("admin");
    users
        .insert(&NewUser {
            id: 81_002,
            email: "author-post@example.com",
            username: "authorpost",
            display_name: "AuthorPost",
            password_hash: Some(&hash),
            role: Role::Author,
            bio: "",
        })
        .await
        .expect("author");
    users
        .insert(&NewUser {
            id: 81_003,
            email: "sub-post@example.com",
            username: "subpost",
            display_name: "SubPost",
            password_hash: Some(&hash),
            role: Role::Subscriber,
            bio: "",
        })
        .await
        .expect("sub");
    users
        .insert(&NewUser {
            id: 81_004,
            email: "contributor-post@example.com",
            username: "contributorpost",
            display_name: "Contributor",
            password_hash: Some(&hash),
            role: Role::Contributor,
            bio: "",
        })
        .await
        .unwrap();
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn posts_crud_and_rbac() {
    let db = TestDb::new().await;
    seed(db.pool()).await;
    let server = TestServer::start(common::BIN, &db);
    let base = server.base();
    let admin = login_token(base, "admin-post@example.com");
    let author = login_token(base, "author-post@example.com");
    let sub = login_token(base, "sub-post@example.com");
    let ck = |t: &str| format!("vy_session={t}");

    let valid_content =
        json!({"schema_version":1,"blocks":[{"kind":"paragraph","attrs":{"text":"hi"}}]});
    let invalid_content =
        json!({"schema_version":1,"blocks":[{"kind":"heading","attrs":{"level":2}}]});

    // create as admin, slug derived
    let first = http(
        ureq::post(format!("{base}/api/v1/posts").as_str())
            .set("cookie", &ck(&admin))
            .send_json(json!({"title":"Hello World","content": valid_content.clone()})),
    );
    assert_eq!(first.status(), 201);
    let first_body = json_body(first);
    assert_eq!(first_body["slug"], "hello-world");
    let first_id = first_body["id"].as_i64().unwrap();

    // duplicate title -> slug -2
    let second = http(
        ureq::post(format!("{base}/api/v1/posts").as_str())
            .set("cookie", &ck(&admin))
            .send_json(json!({"title":"Hello World","content": valid_content.clone()})),
    );
    assert_eq!(second.status(), 201);
    assert_eq!(json_body(second)["slug"], "hello-world-2");

    // author creates own post
    let author_first = http(
        ureq::post(format!("{base}/api/v1/posts").as_str())
            .set("cookie", &ck(&author))
            .send_json(json!({"title":"Dup Title","content": valid_content.clone()})),
    );
    assert_eq!(author_first.status(), 201);
    let author_body = json_body(author_first);
    let author_id = author_body["id"].as_i64().unwrap();
    assert_eq!(author_body["slug"], "dup-title");
    let author_second = http(
        ureq::post(format!("{base}/api/v1/posts").as_str())
            .set("cookie", &ck(&author))
            .send_json(json!({"title":"Dup Title","content": valid_content.clone()})),
    );
    assert_eq!(author_second.status(), 201);
    let author_second = json_body(author_second);
    assert_eq!(author_second["slug"], "dup-title-2");
    let author_second_id = author_second["id"].as_i64().unwrap();

    // validation
    let bad = http(
        ureq::post(format!("{base}/api/v1/posts").as_str())
            .set("cookie", &ck(&admin))
            .send_json(json!({"title":"Bad","content": invalid_content})),
    );
    assert_eq!(bad.status(), 400);

    // subscriber cannot create
    let sub_create = http(
        ureq::post(format!("{base}/api/v1/posts").as_str())
            .set("cookie", &ck(&sub))
            .send_json(json!({"title":"Sub Post","content": valid_content.clone()})),
    );
    assert_eq!(sub_create.status(), 403);

    // list
    let list_admin = json_body(http(
        ureq::get(format!("{base}/api/v1/posts").as_str())
            .set("cookie", &ck(&admin))
            .call(),
    ));
    assert!(list_admin["total"].as_i64().unwrap() >= 4);

    // get by id and by slug
    let get = json_body(http(
        ureq::get(format!("{base}/api/v1/posts/{first_id}").as_str())
            .set("cookie", &ck(&admin))
            .call(),
    ));
    assert_eq!(get["id"], first_id);
    let by_slug = json_body(http(
        ureq::get(format!("{base}/api/v1/posts/slug/post/hello-world").as_str())
            .set("cookie", &ck(&admin))
            .call(),
    ));
    assert_eq!(by_slug["id"], first_id);

    // author cannot edit admin's post
    let forbidden = http(
        ureq::put(format!("{base}/api/v1/posts/{first_id}").as_str())
            .set("cookie", &ck(&author))
            .send_json(json!({"title":"Hacked"})),
    );
    assert_eq!(forbidden.status(), 403);

    // author can edit own
    let ok = http(
        ureq::put(format!("{base}/api/v1/posts/{author_id}").as_str())
            .set("cookie", &ck(&author))
            .send_json(json!({"title":"Updated"})),
    );
    assert_eq!(ok.status(), 200);
    assert_eq!(json_body(ok)["title"], "Updated");

    // trash/restore/delete
    let trash = http(
        ureq::delete(format!("{base}/api/v1/posts/{author_id}").as_str())
            .set("cookie", &ck(&author))
            .call(),
    );
    assert_eq!(trash.status(), 200);
    assert_eq!(json_body(trash)["status"], "trash");
    let restore = http(
        ureq::post(format!("{base}/api/v1/posts/{author_id}/restore").as_str())
            .set("cookie", &ck(&author))
            .call(),
    );
    assert_eq!(restore.status(), 200);
    assert_eq!(json_body(restore)["status"], "draft");
    // hard delete requires delete_posts: author lacks it
    let del_forbidden = http(
        ureq::delete(format!("{base}/api/v1/posts/{author_id}?force=true").as_str())
            .set("cookie", &ck(&author))
            .call(),
    );
    assert_eq!(del_forbidden.status(), 403);
    // admin can hard delete
    let del_admin = http(
        ureq::delete(format!("{base}/api/v1/posts/{author_id}?force=true").as_str())
            .set("cookie", &ck(&admin))
            .call(),
    );
    assert_eq!(del_admin.status(), 204);
    let gone = http(
        ureq::get(format!("{base}/api/v1/posts/{author_id}").as_str())
            .set("cookie", &ck(&admin))
            .call(),
    );
    assert_eq!(gone.status(), 404);
    revision_restore_stays_in_its_post(base, &ck(&admin), &ck(&author), first_id, author_second_id);
    review_authorization(base, &ck(&admin), &ck(&author), &ck(&sub));
}

/// A revision id from someone else's post, restored through a post the
/// caller may edit, must not write into the other post.
fn revision_restore_stays_in_its_post(
    base: &str,
    admin: &str,
    author: &str,
    admin_post: i64,
    author_post: i64,
) {
    let foreign = http(
        ureq::post(format!("{base}/api/v1/posts/{admin_post}/revisions").as_str())
            .set("cookie", admin)
            .send_json(json!({
                "title": "Admin draft",
                "content": {"schema_version":1,"blocks":[]}
            })),
    );
    assert_eq!(foreign.status(), 200);
    let rid = json_body(foreign)["id"].as_i64().unwrap();
    let before = json_body(http(
        ureq::get(format!("{base}/api/v1/posts/{admin_post}").as_str())
            .set("cookie", admin)
            .call(),
    ));
    let attempt = http(
        ureq::post(format!("{base}/api/v1/posts/{author_post}/revisions/{rid}/restore").as_str())
            .set("cookie", author)
            .call(),
    );
    assert_eq!(attempt.status(), 404, "a revision of another post");
    let after = json_body(http(
        ureq::get(format!("{base}/api/v1/posts/{admin_post}").as_str())
            .set("cookie", admin)
            .call(),
    ));
    assert_eq!(after["title"], before["title"]);
    // Through its own post, the same revision restores.
    let own = http(
        ureq::post(format!("{base}/api/v1/posts/{admin_post}/revisions/{rid}/restore").as_str())
            .set("cookie", admin)
            .call(),
    );
    assert_eq!(own.status(), 200);
    assert_eq!(json_body(own)["title"], "Admin draft");
}

#[allow(clippy::too_many_lines)]
fn review_authorization(base: &str, admin: &str, author: &str, subscriber: &str) {
    let content = json!({"schema_version":1,"blocks":[]});
    // Both lookup routes deny private content; its author and editors retain access.
    for (status, password) in [
        ("private", None),
        ("private", Some("open-sesame")),
        ("published", Some("open-sesame")),
    ] {
        let response = http(
            ureq::post(&format!("{base}/api/v1/posts"))
                .set("cookie", author)
                .send_json(json!({
                    "title": format!("Protected {status} {password:?}"), "status": status,
                    "password": password, "content": content
                })),
        );
        assert_eq!(response.status(), 201);
        let post = json_body(response);
        for path in [
            format!("posts/{}", post["id"]),
            format!("posts/slug/post/{}", post["slug"].as_str().unwrap()),
        ] {
            for (cookie, expected) in [(subscriber, 403), (author, 200), (admin, 200)] {
                assert_eq!(
                    http(
                        ureq::get(&format!("{base}/api/v1/{path}"))
                            .set("cookie", cookie)
                            .call()
                    )
                    .status(),
                    expected,
                    "{path}"
                );
            }
        }
        if password.is_some() {
            let verified = http(
                ureq::post(&format!(
                    "{base}/api/v1/posts/{}/verify-password",
                    post["id"]
                ))
                .set("cookie", subscriber)
                .send_json(json!({"password":"open-sesame"})),
            );
            assert_eq!(verified.status(), 200);
            let token = json_body(verified)["token"].as_str().unwrap().to_owned();
            assert_eq!(
                http(
                    ureq::get(&format!("{base}/api/v1/posts/{}?token={token}", post["id"]))
                        .set("cookie", subscriber)
                        .call()
                )
                .status(),
                200
            );
        }
    }
    let listed = json_body(http(
        ureq::get(&format!("{base}/api/v1/posts?status=private"))
            .set("cookie", subscriber)
            .call(),
    ));
    assert_eq!(listed["items"], json!([]));
    assert_eq!(listed["total"], 0);
    let contributor = format!(
        "vy_session={}",
        login_token(base, "contributor-post@example.com")
    );
    let key = json_body(http(
        ureq::post(&format!("{base}/api/v1/api-keys"))
            .set("cookie", admin)
            .send_json(json!({"name":"edit only", "capabilities":["edit_posts"]})),
    ));
    let bearer = format!("Bearer {}", key["key"].as_str().unwrap());
    for (header, credential) in [
        ("cookie", contributor.as_str()),
        ("authorization", bearer.as_str()),
    ] {
        let draft = http(
            ureq::post(&format!("{base}/api/v1/posts"))
                .set(header, credential)
                .send_json(json!({"title":"Allowed draft", "content":content})),
        );
        assert_eq!(draft.status(), 201);
        let id = json_body(draft)["id"].as_i64().unwrap();
        for status in ["published", "scheduled"] {
            let body = json!({"title":"Forbidden publish", "status":status,
                "scheduled_for":"2099-01-01T00:00:00Z", "content":content});
            assert_eq!(
                http(
                    ureq::post(&format!("{base}/api/v1/posts"))
                        .set(header, credential)
                        .send_json(body.clone())
                )
                .status(),
                403
            );
            assert_eq!(
                http(
                    ureq::put(&format!("{base}/api/v1/posts/{id}"))
                        .set(header, credential)
                        .send_json(body)
                )
                .status(),
                403
            );
            for (query, input) in [
                ("mutation($input: CreatePostInput!) { createPost(input:$input) { id } }".to_owned(),
                 json!({"title":"Forbidden GraphQL publish", "content":content,"status":status,"scheduledFor":"2099-01-01T00:00:00Z"})),
                (format!("mutation($input: UpdatePostInput!) {{ updatePost(id:{id}, input:$input) {{ id }} }}"),
                 json!({"status":status,"scheduledFor":"2099-01-01T00:00:00Z"}))
            ] {
                let result = json_body(http(ureq::post(&format!("{base}/api/graphql"))
                    .set(header, credential).send_json(json!({"query":query,"variables":{"input":input}}))));
                assert_eq!(result["errors"][0]["extensions"]["code"], "forbidden", "{result}");
            }
        }
        // Denials must leave the draft untouched.
        let draft = json_body(http(
            ureq::get(&format!("{base}/api/v1/posts/{id}"))
                .set(header, credential)
                .call(),
        ));
        assert_eq!(draft["status"], "draft");
        let batch = json_body(http(
            ureq::post(&format!("{base}/api/v1/posts/batch"))
                .set(header, credential)
                .send_json(json!({"ids":[id],"action":"publish"})),
        ));
        assert_eq!(batch["done"], 0, "{batch}");
    }
    // Publishing remains available to an author with the capability.
    assert_eq!(
        http(
            ureq::post(&format!("{base}/api/v1/posts"))
                .set("cookie", author)
                .send_json(
                    json!({"title":"Allowed publication","status":"published","content":content})
                )
        )
        .status(),
        201
    );
}
