#![allow(clippy::all, clippy::pedantic, clippy::nursery, clippy::restriction)]
#![allow(clippy::unwrap_used, clippy::expect_used)]
#![allow(missing_docs)]
#![allow(unused)]

use std::path::PathBuf;

use async_graphql::dataloader::DataLoader;
use async_graphql::Request;
use vyasa_common::{AiConfig, DbConfig, JobsConfig, LogConfig, Secret, VyasaConfig};
use vyasa_db::content_models::{PostStatus, PostType};
use vyasa_db::models::Role;
use vyasa_db::repo::{NewTerm, NewUser, PostsRepo, TermsRepo, UsersRepo};

use crate::graphql::context::GqlContext;
use crate::graphql::loaders::{
    counters, reset_counters, AuthorLoader, MediaLoader, TermsForPostLoader,
};
use crate::graphql::schema::QueryRoot;
use crate::middleware::auth::CurrentUser;
use crate::middleware::Principal;
use crate::state::AppState;
use vyasa_testkit::TestDb;

/// Serializes this file's tests. `PostService::create` (and the seed
/// helper below, which every test here calls) emits on
/// `vyasa_core::events`, a process-wide bus that is not scoped to a
/// test's own database: without this, `subscription_post_published_and_updated`
/// would intermittently observe another concurrently-running test's
/// published posts instead of (or beside) its own.
static TEST_LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
fn test_lock() -> &'static tokio::sync::Mutex<()> {
    TEST_LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

fn dummy_config() -> VyasaConfig {
    VyasaConfig {
        secret_key: None,
        database_url: Secret::new("postgres://dummy".to_string()),
        bind_addr: "127.0.0.1:0".to_string(),
        media_dir: PathBuf::from("/tmp"),
        index_dir: PathBuf::from("/tmp"),
        registry_dir: PathBuf::from("/tmp"),
        debug: true,
        plugin_trusted_keys: Vec::new(),
        db: DbConfig {
            max_connections: 4,
            acquire_timeout_secs: 5,
        },
        log: LogConfig::default(),
        jobs: JobsConfig::default(),
        ai: AiConfig::default(),
        smtp: None,
        cdn: None,
        storage: None,
        trusted_proxies: Vec::new(),
        trust_cf_connecting_ip: false,
    }
}

#[test]
fn only_queries_are_read_only() {
    use super::is_read_only;
    assert!(is_read_only("{ posts { id } }"));
    assert!(is_read_only("query Q { posts { id } }"));
    assert!(!is_read_only("mutation { deletePost(id: \"1\") }"));
    assert!(!is_read_only("subscription { postPublished { id } }"));
    // A mutation hiding beside a query in one document is still a write:
    // `operationName` picks which runs, and it is the caller's to choose.
    assert!(!is_read_only(
        "query A { posts { id } } mutation B { deletePost(id: \"1\") }"
    ));
    assert!(!is_read_only("not graphql {"));
}

async fn seed_for_graphql(pool: &sqlx::PgPool) -> AppState {
    let users = UsersRepo::new(pool.clone());
    let hash = vyasa_core::user::password::hash_password("pw-secret-1").unwrap();
    // admin
    users
        .insert(&NewUser {
            id: 91_001,
            email: "admin-gql@example.com",
            username: "admingql",
            display_name: "AdminGql",
            password_hash: Some(&hash),
            role: Role::Admin,
            bio: "",
        })
        .await
        .expect("admin");
    // author
    users
        .insert(&NewUser {
            id: 91_002,
            email: "author-gql@example.com",
            username: "authorgql",
            display_name: "AuthorGql",
            password_hash: Some(&hash),
            role: Role::Author,
            bio: "",
        })
        .await
        .expect("author");
    // subscriber
    users
        .insert(&NewUser {
            id: 91_003,
            email: "sub-gql@example.com",
            username: "subgql",
            display_name: "SubGql",
            password_hash: Some(&hash),
            role: Role::Subscriber,
            bio: "",
        })
        .await
        .expect("subscriber");

    // terms
    let terms_repo = TermsRepo::new(pool.clone());
    terms_repo
        .insert(&NewTerm {
            id: 91_010,
            taxonomy: vyasa_db::content_models::Taxonomy::Category,
            name: "Tech",
            slug: "tech",
            parent_id: None,
            meta: serde_json::json!({}),
        })
        .await
        .expect("term");

    let config = dummy_config();
    let state = AppState::new(config, pool.clone());

    // create 3 published posts with same author (admin) and term
    let valid_content = serde_json::json!({"schema_version":1,"blocks":[{"kind":"paragraph","attrs":{"text":"hi"}}]});
    let doc = vyasa_core::block::BlockDocument::from_json(valid_content).expect("valid doc");
    for i in 0..3 {
        let post = state
            .posts
            .create(vyasa_core::post::CreatePost {
                post_type: vyasa_db::content_models::PostType::Post,
                status: vyasa_db::content_models::PostStatus::Published,
                title: format!("GraphQL Post {}", i + 1),
                slug: None,
                content: doc.clone(),
                excerpt: None,
                author_id: 91_001,
                parent_id: None,
                scheduled_for: None,
                password: None,
                term_ids: Some(vec![91_010]),
                layout: None,
            })
            .await
            .expect("create post");
        // Ensure term link (service already did via term_ids, but ensure)
        let _ = post;
    }
    // create one draft post by author (private visible only to author/admin)
    let draft = state
        .posts
        .create(vyasa_core::post::CreatePost {
            post_type: vyasa_db::content_models::PostType::Post,
            status: vyasa_db::content_models::PostStatus::Draft,
            title: "Draft Secret".to_string(),
            slug: None,
            content: doc.clone(),
            excerpt: None,
            author_id: 91_002,
            parent_id: None,
            scheduled_for: None,
            password: None,
            term_ids: None,
            layout: None,
        })
        .await
        .expect("draft");
    assert_eq!(draft.status, vyasa_db::content_models::PostStatus::Draft);
    // set an option for public test
    state
        .options
        .set("site_title", &serde_json::json!("Vyasa GQL"))
        .await
        .expect("set option");

    state
}

#[tokio::test]
async fn posts_with_author_and_terms_batched_once() {
    let _guard = test_lock().lock().await;
    let db = TestDb::new().await;
    let pool = db.pool().clone();
    let state = seed_for_graphql(&pool).await;
    reset_counters();

    // Build schema with loaders
    let schema = crate::graphql::build_schema();
    // Admin principal
    let admin_row = state.users.get(91_001).await.expect("get admin");
    let principal = Principal::Session(CurrentUser {
        token: "tok".to_string(),
        user: admin_row.clone(),
    });
    let ctx = GqlContext {
        state: state.clone(),
        principal: Some(principal),
    };
    let author_loader = DataLoader::new(AuthorLoader::new(pool.clone()), tokio::spawn);
    let terms_loader = DataLoader::new(TermsForPostLoader::new(pool.clone()), tokio::spawn);
    let media_loader = DataLoader::new(MediaLoader::new(pool.clone()), tokio::spawn);

    let query = r#"
    {
        posts(first: 3, status: "published") {
            edges {
                node {
                    id
                    title
                    author {
                        id
                        username
                    }
                    terms {
                        id
                        name
                    }
                    featuredMedia {
                        id
                    }
                }
            }
            totalCount
            pageInfo {
                hasNextPage
                endCursor
            }
        }
    }
    "#;

    let req = Request::new(query)
        .data(ctx.clone())
        .data(author_loader)
        .data(terms_loader)
        .data(media_loader)
        .data(state.clone());

    let resp = schema.execute(req).await;
    assert!(resp.errors.is_empty(), "graphql errors: {:?}", resp.errors);
    let data = resp.data.into_json().expect("json");
    let edges = &data["posts"]["edges"];
    assert!(edges.is_array(), "edges should be array");
    let arr = edges.as_array().unwrap();
    assert_eq!(arr.len(), 3, "expected 3 posts");
    for edge in arr {
        let node = &edge["node"];
        assert!(node["author"]["username"] == "admingql", "author mismatch");
        let terms = node["terms"].as_array().unwrap();
        // Each post has the one term "Tech"
        assert_eq!(terms.len(), 1);
        assert_eq!(terms[0]["name"], "Tech");
    }

    let (a, t, m) = counters();
    assert_eq!(a, 1, "author loader should batch once, got {a}");
    assert_eq!(t, 1, "terms loader should batch once, got {t}");
    // media loader may be 1 if featuredMedia queried (but none have featuredMedia, so 0 or 1?) Our resolver queries media loader only if meta has featured_media_id, none do, so 0
    assert!(m <= 1, "media loader should be at most 1, got {m}");
}

#[tokio::test]
async fn draft_visibility_authz() {
    let _guard = test_lock().lock().await;
    let db = TestDb::new().await;
    let pool = db.pool().clone();
    let state = seed_for_graphql(&pool).await;
    let schema = crate::graphql::build_schema();

    // Helper to execute posts query with given principal
    async fn exec_with_principal(
        schema: &crate::graphql::VyasaSchema,
        state: &AppState,
        pool: &sqlx::PgPool,
        principal: Option<Principal>,
    ) -> serde_json::Value {
        let ctx = GqlContext {
            state: state.clone(),
            principal,
        };
        let req = Request::new(
            r#"{
                posts(first: 10) {
                    edges { node { id title status } }
                }
            }"#,
        )
        .data(ctx)
        .data(DataLoader::new(
            AuthorLoader::new(pool.clone()),
            tokio::spawn,
        ))
        .data(DataLoader::new(
            TermsForPostLoader::new(pool.clone()),
            tokio::spawn,
        ))
        .data(DataLoader::new(
            MediaLoader::new(pool.clone()),
            tokio::spawn,
        ))
        .data(state.clone());
        let resp = schema.execute(req).await;
        assert!(resp.errors.is_empty(), "errors {:?}", resp.errors);
        resp.data.into_json().unwrap()
    }

    // Anonymous should see only published (3)
    let anon_data = exec_with_principal(&schema, &state, &pool, None).await;
    let anon_edges = anon_data["posts"]["edges"].as_array().unwrap();
    assert_eq!(anon_edges.len(), 3, "anon should see 3 published");
    for e in anon_edges {
        assert_eq!(e["node"]["status"], "published");
    }

    // Subscriber (91_003) not author of draft, cannot see draft either (only published)
    let sub_row = state.users.get(91_003).await.unwrap();
    let sub_principal = Principal::Session(CurrentUser {
        token: "t".to_string(),
        user: sub_row,
    });
    let sub_data = exec_with_principal(&schema, &state, &pool, Some(sub_principal)).await;
    let sub_edges = sub_data["posts"]["edges"].as_array().unwrap();
    // Should still be 3 (draft owned by author 91_002, not sub)
    assert_eq!(sub_edges.len(), 3);

    // Author (owner of draft) should see 4 (3 published + own draft)
    let author_row = state.users.get(91_002).await.unwrap();
    let author_principal = Principal::Session(CurrentUser {
        token: "t".to_string(),
        user: author_row,
    });
    let author_data = exec_with_principal(&schema, &state, &pool, Some(author_principal)).await;
    let author_edges = author_data["posts"]["edges"].as_array().unwrap();
    assert_eq!(author_edges.len(), 4, "author should see own draft");

    // Admin (can edit others) should see 4
    let admin_row = state.users.get(91_001).await.unwrap();
    let admin_principal = Principal::Session(CurrentUser {
        token: "t".to_string(),
        user: admin_row,
    });
    let admin_data = exec_with_principal(&schema, &state, &pool, Some(admin_principal)).await;
    let admin_edges = admin_data["posts"]["edges"].as_array().unwrap();
    assert_eq!(admin_edges.len(), 4);
}

#[tokio::test]
async fn me_and_users_require_auth() {
    let _guard = test_lock().lock().await;
    let db = TestDb::new().await;
    let pool = db.pool().clone();
    let state = seed_for_graphql(&pool).await;
    let schema = crate::graphql::build_schema();

    // me without principal should error
    {
        let ctx = GqlContext {
            state: state.clone(),
            principal: None,
        };
        let req = Request::new("{ me { id username } }")
            .data(ctx)
            .data(DataLoader::new(
                AuthorLoader::new(pool.clone()),
                tokio::spawn,
            ))
            .data(DataLoader::new(
                TermsForPostLoader::new(pool.clone()),
                tokio::spawn,
            ))
            .data(DataLoader::new(
                MediaLoader::new(pool.clone()),
                tokio::spawn,
            ))
            .data(state.clone());
        let resp = schema.execute(req).await;
        assert!(!resp.errors.is_empty(), "me without auth should error");
    }
    // me with admin should succeed
    {
        let admin = state.users.get(91_001).await.unwrap();
        let ctx = GqlContext {
            state: state.clone(),
            principal: Some(Principal::Session(CurrentUser {
                token: "t".to_string(),
                user: admin.clone(),
            })),
        };
        let req = Request::new("{ me { id username } }")
            .data(ctx)
            .data(DataLoader::new(
                AuthorLoader::new(pool.clone()),
                tokio::spawn,
            ))
            .data(DataLoader::new(
                TermsForPostLoader::new(pool.clone()),
                tokio::spawn,
            ))
            .data(DataLoader::new(
                MediaLoader::new(pool.clone()),
                tokio::spawn,
            ))
            .data(state.clone());
        let resp = schema.execute(req).await;
        assert!(
            resp.errors.is_empty(),
            "me with auth should pass {:?}",
            resp.errors
        );
        let data = resp.data.into_json().unwrap();
        assert_eq!(data["me"]["username"], "admingql");
    }

    // users without auth should error
    {
        let ctx = GqlContext {
            state: state.clone(),
            principal: None,
        };
        let req = Request::new("{ users(first: 2) { id } }")
            .data(ctx)
            .data(DataLoader::new(
                AuthorLoader::new(pool.clone()),
                tokio::spawn,
            ))
            .data(DataLoader::new(
                TermsForPostLoader::new(pool.clone()),
                tokio::spawn,
            ))
            .data(DataLoader::new(
                MediaLoader::new(pool.clone()),
                tokio::spawn,
            ))
            .data(state.clone());
        let resp = schema.execute(req).await;
        assert!(!resp.errors.is_empty());
    }

    // subscriber cannot list users (needs manage_users)
    {
        let sub = state.users.get(91_003).await.unwrap();
        let ctx = GqlContext {
            state: state.clone(),
            principal: Some(Principal::Session(CurrentUser {
                token: "t".to_string(),
                user: sub,
            })),
        };
        let req = Request::new("{ users(first: 2) { id } }")
            .data(ctx)
            .data(DataLoader::new(
                AuthorLoader::new(pool.clone()),
                tokio::spawn,
            ))
            .data(DataLoader::new(
                TermsForPostLoader::new(pool.clone()),
                tokio::spawn,
            ))
            .data(DataLoader::new(
                MediaLoader::new(pool.clone()),
                tokio::spawn,
            ))
            .data(state.clone());
        let resp = schema.execute(req).await;
        assert!(!resp.errors.is_empty(), "subscriber should be forbidden");
    }

    // admin can list users
    {
        let admin = state.users.get(91_001).await.unwrap();
        let ctx = GqlContext {
            state: state.clone(),
            principal: Some(Principal::Session(CurrentUser {
                token: "t".to_string(),
                user: admin,
            })),
        };
        let req = Request::new("{ users(first: 2) { id } }")
            .data(ctx)
            .data(DataLoader::new(
                AuthorLoader::new(pool.clone()),
                tokio::spawn,
            ))
            .data(DataLoader::new(
                TermsForPostLoader::new(pool.clone()),
                tokio::spawn,
            ))
            .data(DataLoader::new(
                MediaLoader::new(pool.clone()),
                tokio::spawn,
            ))
            .data(state.clone());
        let resp = schema.execute(req).await;
        assert!(
            resp.errors.is_empty(),
            "admin users should pass {:?}",
            resp.errors
        );
        let data = resp.data.into_json().unwrap();
        assert!(data["users"].is_array());
    }
}

#[tokio::test]
async fn public_options_and_media_and_terms() {
    let _guard = test_lock().lock().await;
    let db = TestDb::new().await;
    let pool = db.pool().clone();
    let state = seed_for_graphql(&pool).await;
    let schema = crate::graphql::build_schema();
    let ctx = GqlContext {
        state: state.clone(),
        principal: None,
    };
    // option public
    let req = Request::new(r#"{ option(key: "site_title") }"#)
        .data(ctx.clone())
        .data(DataLoader::new(
            AuthorLoader::new(pool.clone()),
            tokio::spawn,
        ))
        .data(DataLoader::new(
            TermsForPostLoader::new(pool.clone()),
            tokio::spawn,
        ))
        .data(DataLoader::new(
            MediaLoader::new(pool.clone()),
            tokio::spawn,
        ))
        .data(state.clone());
    let resp = schema.execute(req).await;
    assert!(resp.errors.is_empty(), "{:?}", resp.errors);
    let data = resp.data.into_json().unwrap();
    assert_eq!(data["option"], serde_json::json!("Vyasa GQL"));

    // terms query
    let req2 = Request::new(r#"{ terms(taxonomy: "category") { term { id name } postCount } }"#)
        .data(ctx.clone())
        .data(DataLoader::new(
            AuthorLoader::new(pool.clone()),
            tokio::spawn,
        ))
        .data(DataLoader::new(
            TermsForPostLoader::new(pool.clone()),
            tokio::spawn,
        ))
        .data(DataLoader::new(
            MediaLoader::new(pool.clone()),
            tokio::spawn,
        ))
        .data(state.clone());
    let resp2 = schema.execute(req2).await;
    assert!(resp2.errors.is_empty(), "{:?}", resp2.errors);
    let data2 = resp2.data.into_json().unwrap();
    assert_eq!(data2["terms"][0]["term"]["name"], "Tech");

    // media empty initially (no upload yet) but we haven't uploaded media via GraphQL seed; check media query returns empty
    let req3 = Request::new(r#"{ media(first: 1) { id } }"#)
        .data(ctx.clone())
        .data(DataLoader::new(
            AuthorLoader::new(pool.clone()),
            tokio::spawn,
        ))
        .data(DataLoader::new(
            TermsForPostLoader::new(pool.clone()),
            tokio::spawn,
        ))
        .data(DataLoader::new(
            MediaLoader::new(pool.clone()),
            tokio::spawn,
        ))
        .data(state.clone());
    let resp3 = schema.execute(req3).await;
    assert!(resp3.errors.is_empty(), "{:?}", resp3.errors);
}

#[tokio::test]
async fn mutation_create_post_parity_and_auth() {
    let _guard = test_lock().lock().await;
    let db = TestDb::new().await;
    let pool = db.pool().clone();
    let state = seed_for_graphql(&pool).await;
    let schema = crate::graphql::build_schema();

    let admin_row = state.users.get(91_001).await.expect("admin");
    let admin_ctx = GqlContext {
        state: state.clone(),
        principal: Some(Principal::Session(CurrentUser {
            token: "tok".to_string(),
            user: admin_row.clone(),
        })),
    };
    let anon_ctx = GqlContext {
        state: state.clone(),
        principal: None,
    };

    let mk_req = |ctx: GqlContext, query: &str| {
        Request::new(query)
            .data(ctx)
            .data(DataLoader::new(
                AuthorLoader::new(pool.clone()),
                tokio::spawn,
            ))
            .data(DataLoader::new(
                TermsForPostLoader::new(pool.clone()),
                tokio::spawn,
            ))
            .data(DataLoader::new(
                MediaLoader::new(pool.clone()),
                tokio::spawn,
            ))
            .data(state.clone())
    };

    // 1. Unauthenticated createPost should fail with unauthorized code
    let q_unauth = r#"
        mutation {
            createPost(input: { title: "NoAuth", content: {schemaVersion: 1, blocks: [{kind: "paragraph", attrs: {text: "hi"}}]} }) {
                id title
            }
        }
    "#;
    let resp = schema.execute(mk_req(anon_ctx.clone(), q_unauth)).await;
    assert!(!resp.errors.is_empty(), "unauth create should error");
    assert!(
        resp.errors[0].message.contains("authentication")
            || resp.errors[0]
                .extensions
                .as_ref()
                .and_then(|e| e.get("code"))
                .is_some(),
        "should have auth error: {:?}",
        resp.errors
    );

    // 2. Authenticated createPost should succeed and be queryable
    let q_create = r#"
        mutation {
            createPost(input: { title: "GQL Mut Post", content: {schema_version: 1, blocks: [{kind: "paragraph", attrs: {text: "hello"}}]}, status: "published" }) {
                id title status author { username }
            }
        }
    "#;
    let resp = schema.execute(mk_req(admin_ctx.clone(), q_create)).await;
    assert!(
        resp.errors.is_empty(),
        "createPost errors: {:?}",
        resp.errors
    );
    let data = resp.data.into_json().unwrap();
    let post_id = data["createPost"]["id"].as_i64().expect("id");
    assert_eq!(data["createPost"]["title"], "GQL Mut Post");
    assert_eq!(data["createPost"]["status"], "published");
    assert_eq!(data["createPost"]["author"]["username"], "admingql");

    // 3. Query the created post via post(id) as anon (published, should be visible)
    let q_get = format!("{{ post(id: {post_id}) {{ id title }} }}");
    let resp = schema.execute(mk_req(anon_ctx.clone(), &q_get)).await;
    assert!(resp.errors.is_empty(), "get via anon: {:?}", resp.errors);
    let data = resp.data.into_json().unwrap();
    assert_eq!(data["post"]["id"], post_id);

    // 4. UpdatePost as admin (change title)
    let q_update = format!(
        r#"mutation {{ updatePost(id: {post_id}, input: {{ title: "Updated via GQL" }}) {{ id title }} }}"#
    );
    let resp = schema.execute(mk_req(admin_ctx.clone(), &q_update)).await;
    assert!(resp.errors.is_empty(), "update: {:?}", resp.errors);
    let data = resp.data.into_json().unwrap();
    assert_eq!(data["updatePost"]["title"], "Updated via GQL");

    // 5. Trash & restore
    let q_trash = format!("mutation {{ trashPost(id: {post_id}) {{ id status }} }}");
    let resp = schema.execute(mk_req(admin_ctx.clone(), &q_trash)).await;
    assert!(resp.errors.is_empty(), "trash: {:?}", resp.errors);
    assert_eq!(
        resp.data.into_json().unwrap()["trashPost"]["status"],
        "trash"
    );
    let q_restore = format!("mutation {{ restorePost(id: {post_id}) {{ id status }} }}");
    let resp = schema.execute(mk_req(admin_ctx.clone(), &q_restore)).await;
    assert!(resp.errors.is_empty(), "restore: {:?}", resp.errors);
    assert_eq!(
        resp.data.into_json().unwrap()["restorePost"]["status"],
        "draft"
    );

    // 6. Terms: createTerm requires manage_categories (admin has it, author does not)
    let author_row = state.users.get(91_002).await.expect("author");
    let author_ctx = GqlContext {
        state: state.clone(),
        principal: Some(Principal::Session(CurrentUser {
            token: "tok".to_string(),
            user: author_row,
        })),
    };
    let q_term_unauth = r#"
        mutation { createTerm(input: { taxonomy: "category", name: "Nope" }) { id name } }
    "#;
    let resp = schema
        .execute(mk_req(author_ctx.clone(), q_term_unauth))
        .await;
    assert!(
        !resp.errors.is_empty(),
        "author createTerm should be forbidden"
    );

    let q_term = r#"
        mutation { createTerm(input: { taxonomy: "tag", name: "GQLTag" }) { id name taxonomy } }
    "#;
    let resp = schema.execute(mk_req(admin_ctx.clone(), q_term)).await;
    assert!(resp.errors.is_empty(), "createTerm: {:?}", resp.errors);
    let term_id = resp.data.into_json().unwrap()["createTerm"]["id"]
        .as_i64()
        .unwrap();

    // Restore leaves the post a draft, and a draft takes no comments;
    // put it back on the site before a guest can comment on it.
    let q_publish = format!(
        r#"mutation {{ updatePost(id: {post_id}, input: {{ status: "published" }}) {{ id status }} }}"#
    );
    let resp = schema.execute(mk_req(admin_ctx.clone(), &q_publish)).await;
    assert!(resp.errors.is_empty(), "republish: {:?}", resp.errors);

    // 7. Submit comment as guest (no auth) — should succeed
    let q_comment = format!(
        r#"mutation {{ submitComment(input: {{ postId: {post_id}, authorName: "Guest", authorEmail: "guest@example.com", content: "Nice post!" }}) {{ id content }} }}"#
    );
    let resp = schema.execute(mk_req(anon_ctx.clone(), &q_comment)).await;
    assert!(
        resp.errors.is_empty(),
        "submitComment guest: {:?}",
        resp.errors
    );
    let comment_id = resp.data.into_json().unwrap()["submitComment"]["id"]
        .as_i64()
        .unwrap();

    // 8. Moderate comment requires ModerateComments (admin has, author does not)
    let q_mod_unauth = format!(
        "mutation {{ moderateComment(id: {comment_id}, status: \"approved\") {{ id status }} }}"
    );
    let resp = schema
        .execute(mk_req(author_ctx.clone(), &q_mod_unauth))
        .await;
    assert!(!resp.errors.is_empty(), "author moderate should fail");
    let resp = schema
        .execute(mk_req(admin_ctx.clone(), &q_mod_unauth))
        .await;
    assert!(resp.errors.is_empty(), "admin moderate: {:?}", resp.errors);
    assert_eq!(
        resp.data.into_json().unwrap()["moderateComment"]["status"],
        "approved"
    );

    // 9. Upload media via GraphQL should be documented as REST-only (error with validation code)
    let q_upload = r#"mutation { uploadMedia { id } }"#;
    let resp = schema.execute(mk_req(admin_ctx.clone(), q_upload)).await;
    assert!(!resp.errors.is_empty(), "uploadMedia should error");
    let code = resp.errors[0]
        .extensions
        .as_ref()
        .and_then(|e| e.get("code"))
        .and_then(|v| match v {
            async_graphql::Value::String(s) => Some(s.as_str()),
            _ => None,
        });
    assert_eq!(code, Some("validation_failed"));

    // 10. Delete post (hard delete) requires DeletePosts — admin has it
    let q_delete = format!("mutation {{ deletePost(id: {post_id}) {{ success }} }}");
    let resp = schema.execute(mk_req(admin_ctx.clone(), &q_delete)).await;
    assert!(resp.errors.is_empty(), "deletePost: {:?}", resp.errors);
    assert_eq!(
        resp.data.into_json().unwrap()["deletePost"]["success"],
        true
    );
    // Verify gone
    let resp = schema.execute(mk_req(anon_ctx.clone(), &q_get)).await;
    // post(id) returns null when not found (we return Ok(None) for not found, not error)
    let data = resp.data.into_json().unwrap();
    assert!(data["post"].is_null(), "deleted post should be null");

    // Cleanup term
    let q_del_term = format!("mutation {{ deleteTerm(id: {term_id}) {{ success }} }}");
    let resp = schema.execute(mk_req(admin_ctx.clone(), &q_del_term)).await;
    assert!(resp.errors.is_empty());
}

#[tokio::test]
async fn subscription_post_published_and_updated() {
    let _guard = test_lock().lock().await;
    let db = TestDb::new().await;
    let pool = db.pool().clone();
    let state = seed_for_graphql(&pool).await;
    let schema = crate::graphql::build_schema();

    let admin_row = state.users.get(91_001).await.expect("admin");
    let ctx = GqlContext {
        state: state.clone(),
        principal: Some(Principal::Session(CurrentUser {
            token: "tok".to_string(),
            user: admin_row,
        })),
    };

    // Subscribe to postPublished
    let mut stream_published = schema.execute_stream(
        Request::new("subscription { postPublished { postId authorId } }")
            .data(ctx.clone())
            .data(DataLoader::new(
                AuthorLoader::new(pool.clone()),
                tokio::spawn,
            ))
            .data(DataLoader::new(
                TermsForPostLoader::new(pool.clone()),
                tokio::spawn,
            ))
            .data(DataLoader::new(
                MediaLoader::new(pool.clone()),
                tokio::spawn,
            ))
            .data(state.clone()),
    );

    // Subscribe to postUpdated for a specific post id
    let existing_post_id = {
        let posts = state
            .posts
            .list(&vyasa_db::repo::PostFilter {
                status: Some(PostStatus::Published),
                sort: vyasa_db::repo::PostSort::Newest,
                post_type: None,
                author_id: None,
                term_id: None,
                search: None,
                published_month: None,
                limit: 1,
                offset: 0,
                sticky_first: false,
                readable_types: None,
            })
            .await
            .expect("list");
        posts[0].id
    };
    let mut stream_updated = schema.execute_stream(
        Request::new(format!(
            "subscription {{ postUpdated(postId: {existing_post_id}) {{ postId }} }}"
        ))
        .data(ctx.clone())
        .data(DataLoader::new(
            AuthorLoader::new(pool.clone()),
            tokio::spawn,
        ))
        .data(DataLoader::new(
            TermsForPostLoader::new(pool.clone()),
            tokio::spawn,
        ))
        .data(DataLoader::new(
            MediaLoader::new(pool.clone()),
            tokio::spawn,
        ))
        .data(state.clone()),
    );

    use futures_util::StreamExt;
    // Spawn tasks that poll the subscription streams *before* we emit, so the
    // broadcast receivers are registered. If we emitted before the streams were polled,
    // the events would be missed (broadcast only delivers to active receivers).
    // The domain bus is process-wide: another test in this binary that
    // publishes (outside this file's lock) can deliver its event first.
    // Skip foreign events; ours must still arrive within the timeout.
    let ours = std::sync::Arc::new(std::sync::Mutex::new(vec![999_001_i64]));
    let ours_in_task = std::sync::Arc::clone(&ours);
    let handle_published = tokio::spawn(async move {
        let mut s = stream_published;
        loop {
            let item = s.next().await?;
            let id = item
                .data
                .clone()
                .into_json()
                .ok()
                .and_then(|v| v["postPublished"]["postId"].as_i64());
            let mine = ours_in_task
                .lock()
                .map(|o| id.is_some_and(|id| o.contains(&id)));
            if !item.errors.is_empty() || mine.unwrap_or(true) {
                return Some(item);
            }
        }
    });
    let handle_updated = tokio::spawn(async move {
        let mut s = stream_updated;
        s.next().await
    });

    // Give the spawned tasks a moment to be polled and register the receivers.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Emit events via the domain bus
    vyasa_core::events::emit_published(999_001, 91_001);
    vyasa_core::events::emit_updated(existing_post_id);

    // Also trigger a publish via service to ensure event bus works end-to-end
    // (create a scheduled post in the past and publish)
    use vyasa_core::block::{Block, BlockDocument, BlockKind};
    let doc = BlockDocument::new(vec![Block::new(
        BlockKind::Paragraph,
        serde_json::json!({"text": "hi"}),
    )]);
    let past = chrono::Utc::now() - chrono::Duration::seconds(5);
    let sched = state
        .posts
        .create(vyasa_core::post::CreatePost {
            post_type: PostType::Post,
            status: PostStatus::Scheduled,
            title: "SubSched".into(),
            slug: None,
            content: doc,
            excerpt: None,
            author_id: 91_001,
            parent_id: None,
            scheduled_for: Some(past),
            password: None,
            term_ids: None,
            layout: None,
        })
        .await
        .expect("sched");
    ours.lock().expect("ids").push(sched.id);
    let _ = state.posts.publish_due().await;

    // Check published stream received at least our manual emit
    let published = tokio::time::timeout(std::time::Duration::from_secs(2), handle_published)
        .await
        .expect("timeout published")
        .expect("join")
        .expect("stream item");
    assert!(
        published.errors.is_empty(),
        "published errors: {:?}",
        published.errors
    );
    let val = published.data.into_json().unwrap();
    // Could be 999_001 or sched.id, both are Published events
    let published_id = val["postPublished"]["postId"].as_i64().unwrap();
    assert!(
        published_id == 999_001 || published_id == sched.id,
        "published_id {published_id}"
    );

    // Check updated stream
    let updated = tokio::time::timeout(std::time::Duration::from_secs(2), handle_updated)
        .await
        .expect("timeout updated")
        .expect("join")
        .expect("stream item");
    assert!(updated.errors.is_empty());
    let val = updated.data.into_json().unwrap();
    assert_eq!(val["postUpdated"]["postId"], existing_post_id);
}

#[tokio::test]
async fn protected_content_and_key_grants_are_enforced_before_pagination() {
    let _guard = test_lock().lock().await;
    let db = TestDb::new().await;
    let pool = db.pool().clone();
    let mut state = seed_for_graphql(&pool).await;
    let index_dir = tempfile::tempdir().unwrap();
    let index = std::sync::Arc::new(vyasa_search::IndexManager::open(index_dir.path()).unwrap());
    state.search_index = Some(index.clone());
    let schema = crate::graphql::build_schema();
    sqlx::query("UPDATE posts SET password_hash = 'protected' WHERE slug = 'graphql-post-1'")
        .execute(&pool)
        .await
        .unwrap();
    let protected = state
        .posts
        .get_by_slug(PostType::Post, "graphql-post-1")
        .await
        .unwrap();
    index
        .upsert(&vyasa_search::index::SearchDoc {
            id: protected.id as u64,
            post_type: "post".into(),
            slug: protected.slug.clone(),
            title: "Confidential".into(),
            body: "Confidential protected body".into(),
        })
        .unwrap();
    assert_eq!(index.search("Confidential", None, 10, 0).unwrap().len(), 1);
    // A stale index must not expose protected body snippets over REST either.
    let axum::Json(result) = crate::rest::search::search(
        axum::extract::State(state.clone()),
        crate::middleware::principal::MaybePrincipal(None),
        axum::extract::Query(crate::rest::search::SearchParams {
            q: "Confidential".into(),
            limit: None,
        }),
    )
    .await
    .unwrap_or_else(|e| panic!("REST search: {}", e.0));
    assert_eq!(result["hits"], serde_json::json!([]));
    crate::search_indexer::reindex_all(&state).await.unwrap();
    assert!(index
        .search("Confidential", None, 10, 0)
        .unwrap()
        .is_empty());
    assert!(index
        .search("GraphQL", None, 10, 0)
        .unwrap()
        .iter()
        .all(|hit| hit.id != protected.id as u64));

    // More than the former 1,000-row ceiling, including invisible drafts.
    sqlx::query(
        "INSERT INTO posts (id, type, status, slug, title, content, author_id, created_at)
        SELECT 800000 + n, 'post', CASE WHEN n % 2 = 0 THEN 'published' ELSE 'draft' END,
        'bulk-' || n, 'Bulk ' || n, '{}'::jsonb, 91002, now() + interval '1 second'
        FROM generate_series(1, 1100) n",
    )
    .execute(&pool)
    .await
    .unwrap();
    let user = state.users.get(91001).await.unwrap();
    let restricted = Principal::ApiKey(crate::middleware::api_key::ApiKeyPrincipal {
        key: vyasa_db::models::ApiKeyRow {
            id: 999,
            user_id: user.id,
            name: "restricted".into(),
            key_hash: "unused".into(),
            capabilities: serde_json::json!([]),
            last_used_at: None,
            created_at: chrono::Utc::now(),
            revoked_at: None,
        },
        user,
    });
    for principal in [None, Some(restricted.clone())] {
        let context = GqlContext {
            state: state.clone(),
            principal,
        };
        assert!(!context.can(vyasa_core::user::Capability::ManageUsers));
        let response = schema.execute(Request::new(r#"{
            posts(first: 10, after: "NTUw") {
                totalCount edges { node { title content } }
                pageInfo { hasNextPage }
            }
            search(query: "Confidential") { snippet post { content } }
            drafts: posts(status: "draft") { totalCount edges { node { id } } pageInfo { hasNextPage } }
        }"#).data(context.clone())).await;
        assert!(response.errors.is_empty(), "{:?}", response.errors);
        let data = response.data.into_json().unwrap();
        assert_eq!(data["search"], serde_json::json!([]));
        assert_eq!(data["posts"]["totalCount"], 552);
        assert_eq!(data["posts"]["edges"].as_array().unwrap().len(), 2);
        assert_eq!(data["posts"]["pageInfo"]["hasNextPage"], false);
        assert_eq!(data["drafts"]["totalCount"], 0);
        assert_eq!(data["drafts"]["pageInfo"]["hasNextPage"], false);
        for slug in ["graphql-post-1", "draft-secret"] {
            let response = schema
                .execute(
                    Request::new(format!(
                        "{{ postBySlug(postType: \"post\", slug: \"{slug}\") {{ content }} }}"
                    ))
                    .data(context.clone()),
                )
                .await;
            assert!(!response.errors.is_empty(), "must deny {slug}");
        }
        let author = state.users.get(91002).await.unwrap();
        // Exercise the nested author type directly, avoiding unrelated loaders.
        let author_schema = async_graphql::Schema::build(
            crate::graphql::types::user::GqlUser(author),
            async_graphql::EmptyMutation,
            async_graphql::EmptySubscription,
        )
        .finish();
        let response = author_schema
            .execute(Request::new("{ email }").data(context))
            .await;
        assert!(response.errors.is_empty());
        assert_eq!(response.data.into_json().unwrap()["email"], "");
    }
    let context = GqlContext {
        state: state.clone(),
        principal: Some(Principal::Session(CurrentUser {
            token: "test".into(),
            user: state.users.get(91001).await.unwrap(),
        })),
    };
    let response = schema
        .execute(
            Request::new(
                "{ postBySlug(postType: \"post\", slug: \"graphql-post-1\") { content } }",
            )
            .data(context),
        )
        .await;
    assert!(
        response.errors.is_empty(),
        "editors retain protected access"
    );
}

#[tokio::test]
async fn recovery_codes_are_consumed_atomically_under_concurrent_requests() {
    use sha2::{Digest, Sha256};
    let _guard = test_lock().lock().await;
    let db = TestDb::new().await;
    let pool = db.pool().clone();
    let state = seed_for_graphql(&pool).await;
    crate::mfa::begin(&state, 91001, "admin-gql@example.com")
        .await
        .unwrap();
    let codes = ["aaaaaaaaaa", "bbbbbbbbbb", "cccccccccc"];
    let hashes: Vec<String> = codes
        .iter()
        .map(|c| hex::encode(Sha256::digest(c.as_bytes())))
        .collect();
    for same_code in [true, false] {
        sqlx::query(
            "UPDATE user_mfa SET enabled_at = now(), recovery_codes = $1 WHERE user_id = 91001",
        )
        .bind(serde_json::json!(hashes))
        .execute(&pool)
        .await
        .unwrap();
        let mut tx = pool.begin().await.unwrap();
        sqlx::query("SELECT user_id FROM user_mfa WHERE user_id = 91001 FOR UPDATE")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        let tasks: Vec<_> = [codes[0], if same_code { codes[0] } else { codes[1] }]
            .into_iter()
            .map(|code| {
                let state = state.clone();
                tokio::spawn(async move {
                    crate::mfa::verify(&state, 91001, "admin-gql@example.com", code).await
                })
            })
            .collect();
        // Hold the row until both requests have read it and attempted to
        // consume their code, reproducing the lost-update race reliably.
        let waiting = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                let n: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE datname = current_database() AND wait_event_type = 'Lock' AND query LIKE 'UPDATE user_mfa SET recovery_codes%'")
                    .fetch_one(&pool).await.unwrap();
                if n == 2 { break; }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        }).await;
        tx.commit().await.unwrap();
        assert!(
            waiting.is_ok(),
            "both requests must reach the locked update"
        );
        let mut accepted = 0;
        for task in tasks {
            accepted += usize::from(task.await.unwrap().is_ok());
        }
        assert_eq!(accepted, if same_code { 1 } else { 2 });
        let remaining: serde_json::Value =
            sqlx::query_scalar("SELECT recovery_codes FROM user_mfa WHERE user_id = 91001")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            remaining.as_array().unwrap().len(),
            if same_code { 2 } else { 1 }
        );
        assert!(!remaining
            .as_array()
            .unwrap()
            .contains(&serde_json::json!(hashes[0])));
        assert!(
            crate::mfa::verify(&state, 91001, "admin-gql@example.com", codes[0])
                .await
                .is_err()
        );
        assert!(
            crate::mfa::verify(&state, 91001, "admin-gql@example.com", codes[2])
                .await
                .is_ok()
        );
    }
}

#[tokio::test]
async fn integrated_ai_backfill_and_site_timezone() {
    let _guard = test_lock().lock().await;
    let db = TestDb::new().await;
    let pool = db.pool().clone();
    let state = seed_for_graphql(&pool).await;
    let admin = Principal::Session(CurrentUser {
        token: "test".into(),
        user: state.users.get(91001).await.unwrap(),
    });
    let subscriber = Principal::Session(CurrentUser {
        token: "test".into(),
        user: state.users.get(91003).await.unwrap(),
    });
    state
        .options_service
        .put("ai_embeddings", serde_json::json!(false))
        .await
        .unwrap();
    let request = || crate::rest::ai_features::BackfillRequest {
        feature: "embeddings".into(),
        after_id: None,
    };
    assert!(crate::rest::ai_features::backfill(
        axum::extract::State(state.clone()),
        admin.clone(),
        axum::Json(request())
    )
    .await
    .is_err());
    state
        .options_service
        .put("ai_embeddings", serde_json::json!(true))
        .await
        .unwrap();
    assert!(crate::rest::ai_features::backfill(
        axum::extract::State(state.clone()),
        subscriber,
        axum::Json(request())
    )
    .await
    .is_err());
    sqlx::query("INSERT INTO posts (id, type, status, slug, title, content, author_id) SELECT 810000+n, 'post', 'published', 'backfill-'||n, 'Backfill '||n, '{}'::jsonb, 91001 FROM generate_series(1,105) n").execute(&pool).await.unwrap();
    let axum::Json(first) = crate::rest::ai_features::backfill(
        axum::extract::State(state.clone()),
        admin.clone(),
        axum::Json(request()),
    )
    .await
    .unwrap_or_else(|e| panic!("{}", e.0));
    assert_eq!(first["queued"], 100);
    let after = first["next_after_id"].as_str().unwrap().parse().unwrap();
    let axum::Json(second) = crate::rest::ai_features::backfill(
        axum::extract::State(state.clone()),
        admin.clone(),
        axum::Json(crate::rest::ai_features::BackfillRequest {
            after_id: Some(after),
            ..request()
        }),
    )
    .await
    .unwrap_or_else(|e| panic!("{}", e.0));
    assert_eq!(second["queued"], 8);
    assert!(second["next_after_id"].is_null());
    let axum::Json(repeated) = crate::rest::ai_features::backfill(
        axum::extract::State(state.clone()),
        admin.clone(),
        axum::Json(request()),
    )
    .await
    .unwrap_or_else(|e| panic!("{}", e.0));
    assert_eq!(repeated["queued"], 0);
    let (count, distinct): (i64,i64) = sqlx::query_as("SELECT count(*), count(DISTINCT payload->>'post_id') FROM jobs WHERE kind = 'ai_embed_post'").fetch_one(&pool).await.unwrap();
    assert_eq!((count, distinct), (108, 108));
    state
        .options_service
        .put("timezone", serde_json::json!("America/Los_Angeles"))
        .await
        .unwrap();
    state
        .options_service
        .put("date_format", serde_json::json!("%Y-%m-%d %H:%M"))
        .await
        .unwrap();
    let site = crate::public::page_meta::SiteContext::load(&state, "").await;
    let at = chrono::DateTime::parse_from_rfc3339("2026-01-01T00:30:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    assert_eq!(site.date(at), "2025-12-31 16:30");
    assert!(state
        .options_service
        .put("timezone", serde_json::json!("invalid-zone"))
        .await
        .is_err());
    state
        .options_service
        .put("timezone", serde_json::json!("UTC"))
        .await
        .unwrap();
    state
        .options_service
        .put("ai_embeddings", serde_json::json!(false))
        .await
        .unwrap();
}

/// A page of entries reads its type's field definitions once per request,
/// not once per entry (`fields` and `fieldsMissing` share the cache).
#[tokio::test]
async fn field_definitions_are_read_once_per_request() {
    let _guard = test_lock().lock().await;
    let db = TestDb::new().await;
    let pool = db.pool().clone();
    let state = seed_for_graphql(&pool).await;
    vyasa_core::content::ContentTypesService::new(pool.clone())
        .create(
            vyasa_core::content::NewType {
                slug: "gql-cache-thing".into(),
                singular: "Thing".into(),
                plural: "Things".into(),
                description: String::new(),
                public: true,
                has_archive: true,
            },
            &[] as &[String],
        )
        .await
        .expect("type");
    vyasa_core::content::ContentFieldsService::new(pool.clone())
        .create(
            "gql-cache-thing",
            vyasa_core::content::NewField {
                key: "colour".into(),
                label: "Colour".into(),
                help: String::new(),
                kind: vyasa_core::content::FieldKind::Text,
                required: false,
                options: serde_json::json!({}),
            },
        )
        .await
        .expect("field");
    let thing = PostType::parse("gql-cache-thing").expect("live");
    for n in 0..3 {
        state
            .posts
            .create_with_fields(
                vyasa_core::post::CreatePost {
                    post_type: thing,
                    status: PostStatus::Published,
                    title: format!("Thing {n}"),
                    slug: None,
                    content: vyasa_core::block::BlockDocument::new(Vec::new()),
                    excerpt: None,
                    author_id: 91_001,
                    parent_id: None,
                    scheduled_for: None,
                    password: None,
                    term_ids: None,
                    layout: None,
                },
                Some(serde_json::json!({"colour": format!("c{n}")})),
            )
            .await
            .expect("entry");
    }
    let admin_row = state.users.get(91_001).await.expect("get admin");
    let ctx = GqlContext {
        state: state.clone(),
        principal: Some(Principal::Session(CurrentUser {
            token: "tok".to_string(),
            user: admin_row,
        })),
    };
    let shared = std::sync::Arc::new(crate::entry_fields::SharedDefinitions::default());
    let req = Request::new(
        r#"{ posts(first: 10, postType: "gql-cache-thing") { edges { node { fields fieldsMissing } } } }"#,
    )
    .data(ctx)
    .data(std::sync::Arc::clone(&shared))
    .data(state.clone());
    let resp = crate::graphql::build_schema().execute(req).await;
    assert!(resp.errors.is_empty(), "{:?}", resp.errors);
    let data = resp.data.into_json().expect("json");
    let edges = data["posts"]["edges"].as_array().expect("edges");
    assert_eq!(edges.len(), 3);
    for edge in edges {
        assert!(edge["node"]["fields"]["colour"].is_string(), "{edge}");
        assert_eq!(edge["node"]["fieldsMissing"], serde_json::json!([]));
    }
    assert_eq!(
        shared.loads(),
        1,
        "one read of the definitions for three entries"
    );
}

/// Each operation gets its own field-definitions cache (a WebSocket
/// connection runs many operations; one cache per connection would serve
/// definitions from when it connected).
#[test]
fn every_operation_gets_its_own_field_definitions_cache() {
    use std::any::TypeId;
    let key = TypeId::of::<std::sync::Arc<crate::entry_fields::SharedDefinitions>>();
    let fresh = super::with_fresh_definitions(Request::new("{ posts { edges { node { id } } } }"));
    assert!(fresh.data.contains_key(&key));
    // Two operations, two caches.
    let a = super::with_fresh_definitions(Request::new("{ a: posts { totalCount } }"));
    let b = super::with_fresh_definitions(Request::new("{ b: posts { totalCount } }"));
    let ptr = |r: &Request| {
        r.data
            .get(&key)
            .and_then(|v| {
                v.downcast_ref::<std::sync::Arc<crate::entry_fields::SharedDefinitions>>()
            })
            .map(|a| std::sync::Arc::as_ptr(a) as usize)
    };
    assert_ne!(ptr(&a), ptr(&b));
    // One supplied by the caller is kept.
    let mine = std::sync::Arc::new(crate::entry_fields::SharedDefinitions::default());
    let kept = super::with_fresh_definitions(
        Request::new("{ posts { totalCount } }").data(std::sync::Arc::clone(&mine)),
    );
    assert_eq!(ptr(&kept), Some(std::sync::Arc::as_ptr(&mine) as usize));
}
