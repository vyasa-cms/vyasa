//! Hierarchical post types over HTTP: REST, GraphQL, public addresses, search
//! scopes, managed entries. Tasks 3 to 6, 10, 12 and 17 add tests here.
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::too_many_lines,
    dead_code
)]

mod common;

use serde_json::{json, Value};
use vyasa_db::models::Role;
use vyasa_testkit::{TestDb, TestServer};

fn http(r: Result<ureq::Response, ureq::Error>) -> ureq::Response {
    match r {
        Ok(v) | Err(ureq::Error::Status(_, v)) => v,
        Err(ureq::Error::Transport(e)) => panic!("transport {e}"),
    }
}

fn text(r: ureq::Response) -> String {
    use std::io::Read as _;
    let mut s = String::new();
    r.into_reader()
        .take(8_000_000)
        .read_to_string(&mut s)
        .unwrap();
    s
}

fn json_of(r: ureq::Response) -> Value {
    let body = text(r);
    serde_json::from_str(&body).unwrap_or_else(|e| panic!("not JSON ({e}): {body}"))
}

/// A request with a session cookie and a JSON body; returns status and body.
fn call(method: &str, base: &str, cookie: &str, path: &str, body: Option<Value>) -> (u16, Value) {
    let req = ureq::request(method, &format!("{base}{path}")).set("cookie", cookie);
    let resp = http(match body {
        Some(b) => req.send_json(b),
        None => req.call(),
    });
    let status = resp.status();
    let body = text(resp);
    let value = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_str(&body).unwrap_or(Value::String(body))
    };
    (status, value)
}

fn get_public(base: &str, path: &str) -> (u16, String) {
    let resp = http(ureq::get(&format!("{base}{path}")).call());
    (resp.status(), text(resp))
}

/// An id as text, whether the response carries it as a string or a number
/// (post responses have always carried numbers; the tree surfaces use strings).
fn sid(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn doc(words: &str) -> Value {
    json!({"schema_version": 1, "blocks": [{"kind": "paragraph", "attrs": {"text": words}}]})
}

/// Creates a type and asserts it was created.
fn create_type(base: &str, cookie: &str, slug: &str, singular: &str, plural: &str) {
    let (status, body) = call(
        "POST",
        base,
        cookie,
        "/api/v1/content-types",
        Some(json!({"slug": slug, "singular": singular, "plural": plural})),
    );
    assert_eq!(status, 201, "create type {slug}: {body}");
}

#[tokio::test]
async fn an_administrator_can_make_a_type_hierarchical_and_back() {
    let db = TestDb::new().await;
    let admin = common::seed_user(db.pool(), Role::Admin).await;
    let server = TestServer::start(common::BIN, &db);
    let base = server.base();
    let a = common::login_cookie(base, &admin.email, &admin.password);
    let (status, body) = call(
        "POST",
        base,
        &a,
        "/api/v1/content-types",
        Some(
            json!({"slug": "guide", "singular": "Guide", "plural": "Guides", "hierarchical": true}),
        ),
    );
    assert_eq!(status, 201, "{body}");
    let (_, list) = call("GET", base, &a, "/api/v1/content-types", None);
    let guide = list
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["slug"] == "guide")
        .expect("listed");
    assert_eq!(guide["hierarchical"], true);
    assert_eq!(guide["base"], "guide");
    // A post type that is not hierarchical stays flat.
    create_type(base, &a, "note", "Note", "Notes");
    let (_, list) = call("GET", base, &a, "/api/v1/content-types", None);
    assert_eq!(
        list.as_array()
            .unwrap()
            .iter()
            .find(|t| t["slug"] == "note")
            .unwrap()["hierarchical"],
        false
    );
    // Turning it off with nested entries is refused; flat entries are fine.
    let (_, parent) = call(
        "POST",
        base,
        &a,
        "/api/v1/posts",
        Some(
            json!({"type": "guide", "title": "Parent", "status": "published", "content": doc("p")}),
        ),
    );
    let pid = sid(&parent["id"]);
    let (status, child) = call(
        "POST",
        base,
        &a,
        "/api/v1/posts",
        Some(
            json!({"type": "guide", "title": "Child", "status": "published", "parent_id": pid, "content": doc("c")}),
        ),
    );
    assert_eq!(status, 201, "{child}");
    let (status, body) = call(
        "PUT",
        base,
        &a,
        "/api/v1/content-types/guide",
        Some(json!({"hierarchical": false})),
    );
    assert_eq!(status, 400, "{body}");
    assert!(
        body["message"].as_str().unwrap().contains("parents"),
        "{body}"
    );
}

fn post(base: &str, cookie: &str, ty: &str, title: &str, parent: Option<&str>) -> String {
    let mut body =
        json!({"type": ty, "title": title, "status": "published", "content": doc(title)});
    if let Some(p) = parent {
        body["parent_id"] = json!(p);
    }
    let (status, v) = call("POST", base, cookie, "/api/v1/posts", Some(body));
    assert_eq!(status, 201, "create {title}: {v}");
    sid(&v["id"])
}

async fn site() -> (TestDb, TestServer, String) {
    let db = TestDb::new().await;
    let admin = common::seed_user(db.pool(), Role::Admin).await;
    let server = TestServer::start(common::BIN, &db);
    let a = common::login_cookie(server.base(), &admin.email, &admin.password);
    let (status, body) = call(
        "POST",
        server.base(),
        &a,
        "/api/v1/content-types",
        Some(
            json!({"slug": "guide", "singular": "Guide", "plural": "Guides", "hierarchical": true}),
        ),
    );
    assert_eq!(status, 201, "{body}");
    (db, server, a)
}

#[tokio::test]
async fn entries_report_position_and_path_and_the_tree_is_in_reading_order() {
    let (_db, server, a) = site().await;
    let base = server.base();
    let guides = post(base, &a, "guide", "Guides", None);
    let install = post(base, &a, "guide", "Install", Some(&guides));
    let intro = post(base, &a, "guide", "Intro", None);
    let (_, one) = call("GET", base, &a, &format!("/api/v1/posts/{install}"), None);
    assert_eq!(one["path"], "guides/install");
    assert_eq!(sid(&one["parent_id"]), guides);
    let (status, tree) = call(
        "GET",
        base,
        &a,
        "/api/v1/post-types/guide/tree?status=all",
        None,
    );
    assert_eq!(status, 200, "{tree}");
    let titles: Vec<_> = tree["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| (n["title"].as_str().unwrap(), n["depth"].as_i64().unwrap()))
        .collect();
    assert_eq!(titles, [("Guides", 1), ("Install", 2), ("Intro", 1)]);
    assert_eq!(tree["nodes"][1]["url"], "/guide/guides/install");
    assert_eq!(tree["nodes"][0]["id"], guides);
    let _ = intro;
}

#[tokio::test]
async fn the_public_tree_shows_only_published_readable_entries() {
    let (_db, server, a) = site().await;
    let base = server.base();
    let pub_id = post(base, &a, "guide", "Public", None);
    let (_, draft) = call(
        "POST",
        base,
        &a,
        "/api/v1/posts",
        Some(json!({"type": "guide", "title": "Draft", "status": "draft", "content": doc("d")})),
    );
    let _ = (pub_id, draft);
    let resp = http(ureq::get(&format!("{base}/api/v1/post-types/guide/tree")).call());
    assert_eq!(resp.status(), 200);
    let tree = json_of(resp);
    let titles: Vec<_> = tree["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["title"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(titles, ["Public"]);
    // `status=all` needs a session that edits posts.
    let resp = http(ureq::get(&format!("{base}/api/v1/post-types/guide/tree?status=all")).call());
    assert_eq!(resp.status(), 403);
    // A type that is not hierarchical has no tree.
    let (status, _) = call("GET", base, &a, "/api/v1/post-types/post/tree", None);
    assert_eq!(status, 400);
}

#[tokio::test]
async fn put_parent_and_position_move_an_entry_and_null_makes_it_a_root() {
    let (_db, server, a) = site().await;
    let base = server.base();
    let x = post(base, &a, "guide", "X", None);
    let y = post(base, &a, "guide", "Y", None);
    let (status, moved) = call(
        "PUT",
        base,
        &a,
        &format!("/api/v1/posts/{y}"),
        Some(json!({"parent_id": x, "position": 5})),
    );
    assert_eq!(status, 200, "{moved}");
    assert_eq!(
        (moved["path"].as_str(), moved["position"].as_i64()),
        (Some("x/y"), Some(5))
    );
    // Omitting parent_id keeps it; null clears it.
    let (_, kept) = call(
        "PUT",
        base,
        &a,
        &format!("/api/v1/posts/{y}"),
        Some(json!({"title": "Y2"})),
    );
    assert_eq!(sid(&kept["parent_id"]), x);
    let (_, root) = call(
        "PUT",
        base,
        &a,
        &format!("/api/v1/posts/{y}"),
        Some(json!({"parent_id": null})),
    );
    assert_eq!(root["parent_id"], Value::Null);
    assert_eq!(root["path"], "y");
}

#[tokio::test]
async fn the_tree_put_applies_moves_whole_and_a_swap_in_one_request_succeeds() {
    let (_db, server, a) = site().await;
    let base = server.base();
    let p = post(base, &a, "guide", "P", None);
    let q = post(base, &a, "guide", "Q", None);
    let xp = post(base, &a, "guide", "X", Some(&p));
    let xq = post(base, &a, "guide", "X", Some(&q));
    let (status, tree) = call(
        "PUT",
        base,
        &a,
        "/api/v1/post-types/guide/tree",
        Some(
            json!({"moves": [{"id": xp, "parent_id": q, "position": 10}, {"id": xq, "parent_id": p, "position": 10}]}),
        ),
    );
    assert_eq!(status, 200, "{tree}");
    let paths: std::collections::BTreeMap<String, String> = tree["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| {
            (
                n["id"].as_str().unwrap().to_owned(),
                n["path"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    assert_eq!(paths[&xp], "q/x");
    assert_eq!(paths[&xq], "p/x");
    // A loop is a 400 naming the rule, and nothing moves.
    let (status, body) = call(
        "PUT",
        base,
        &a,
        "/api/v1/post-types/guide/tree",
        Some(json!({"moves": [{"id": p, "parent_id": xq, "position": 1}]})),
    );
    assert_eq!(status, 400, "{body}");
    assert!(
        body["message"].as_str().unwrap().contains("ancestor"),
        "{body}"
    );
    // An id that is not an entry of the type is a 404.
    let (status, _) = call(
        "PUT",
        base,
        &a,
        "/api/v1/post-types/guide/tree",
        Some(json!({"moves": [{"id": "1", "parent_id": null, "position": 1}]})),
    );
    assert_eq!(status, 404);
}

#[tokio::test]
async fn moving_a_published_entry_leaves_a_redirect_from_its_old_address() {
    let (_db, server, a) = site().await;
    let base = server.base();
    let p = post(base, &a, "guide", "P", None);
    let q = post(base, &a, "guide", "Q", None);
    let x = post(base, &a, "guide", "X", Some(&p));
    let (status, _) = call(
        "PUT",
        base,
        &a,
        "/api/v1/post-types/guide/tree",
        Some(json!({"moves": [{"id": x, "parent_id": q, "position": 10}]})),
    );
    assert_eq!(status, 200);
    let (_, redirects) = call("GET", base, &a, "/api/v1/redirects", None);
    let found = redirects
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r["from_path"] == "/guide/p/x" && r["to_path"] == "/guide/q/x");
    assert!(found, "{redirects}");
}

#[tokio::test]
async fn editors_without_edit_others_cannot_move_other_peoples_entries() {
    let (db, server, a) = site().await;
    let base = server.base();
    let author = common::seed_user(db.pool(), Role::Author).await;
    let c = common::login_cookie(base, &author.email, &author.password);
    let theirs = post(base, &a, "guide", "Admin's", None);
    let (status, _) = call(
        "PUT",
        base,
        &c,
        "/api/v1/post-types/guide/tree",
        Some(json!({"moves": [{"id": theirs, "parent_id": null, "position": 99}]})),
    );
    assert_eq!(status, 403);
}

#[tokio::test]
async fn export_then_import_keeps_a_hierarchical_tree_with_cousins_and_order() {
    let (source_db, source, a) = site().await;
    let base = source.base();
    let g = post(base, &a, "guide", "Guides", None);
    let api = post(base, &a, "guide", "API", None);
    post(base, &a, "guide", "Install", Some(&g));
    post(base, &a, "guide", "Install", Some(&api));
    // API sorts first by position, not by creation.
    call(
        "PUT",
        base,
        &a,
        &format!("/api/v1/posts/{api}"),
        Some(json!({"position": 1})),
    );
    let resp = http(
        ureq::get(&format!("{base}/api/v1/export?format=json"))
            .set("cookie", &a)
            .call(),
    );
    assert_eq!(resp.status(), 200);
    let archive = json_of(resp);
    assert_eq!(archive["content_types"][0]["hierarchical"], true);
    drop(source);
    drop(source_db);

    let target_db = TestDb::new().await;
    let admin = common::seed_user(target_db.pool(), Role::Admin).await;
    let target = TestServer::start(common::BIN, &target_db);
    let base = target.base();
    let t = common::login_cookie(base, &admin.email, &admin.password);
    let (status, report) = call("POST", base, &t, "/api/v1/import", Some(archive));
    assert_eq!(status, 200, "{report}");
    assert!(
        report["warnings"].as_array().unwrap().is_empty(),
        "{report}"
    );
    let (_, tree) = call(
        "GET",
        base,
        &t,
        "/api/v1/post-types/guide/tree?status=all",
        None,
    );
    let paths: Vec<_> = tree["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["path"].as_str().unwrap())
        .collect();
    assert_eq!(paths, ["api", "api/install", "guides", "guides/install"]);
    assert_eq!(get_public(base, "/guide/api/install").0, 200);
    assert_eq!(get_public(base, "/guide/guides/install").0, 200);
}

#[tokio::test]
async fn graphql_exposes_position_path_parent_children_and_the_tree() {
    let (_db, server, a) = site().await;
    let base = server.base();
    let g = post(base, &a, "guide", "Guides", None);
    let i = post(base, &a, "guide", "Install", Some(&g));
    let (_, draft) = call(
        "POST",
        base,
        &a,
        "/api/v1/posts",
        Some(
            json!({"type": "guide", "title": "Hidden", "status": "draft", "parent_id": g, "content": doc("h")}),
        ),
    );
    let _ = (i, draft);
    let q = json!({"query": format!("{{ post(id: {g}) {{ path position children {{ title path parent {{ title }} }} }}
        postTree(postType: \"guide\") {{ depth parentId post {{ title }} }} }}")});
    let (status, body) = call("POST", base, &a, "/api/graphql", Some(q));
    assert_eq!(status, 200, "{body}");
    let data = &body["data"];
    assert_eq!(data["post"]["path"], "guides");
    let kids = data["post"]["children"].as_array().unwrap();
    assert_eq!(
        kids.len(),
        1,
        "the draft is not a visible child even to its editor: {data}"
    );
    assert_eq!(kids[0]["title"], "Install");
    assert_eq!(kids[0]["parent"]["title"], "Guides");
    let tree: Vec<_> = data["postTree"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| {
            (
                n["post"]["title"].as_str().unwrap(),
                n["depth"].as_i64().unwrap(),
            )
        })
        .collect();
    assert_eq!(tree, [("Guides", 1), ("Install", 2)]);
    // Anonymous callers see the same published tree.
    let resp = http(
        ureq::post(&format!("{base}/api/graphql"))
            .send_json(json!({"query": "{ postTree(postType: \"guide\") { post { title } } }"})),
    );
    assert_eq!(
        json_of(resp)["data"]["postTree"].as_array().unwrap().len(),
        2
    );
}

#[tokio::test]
async fn nested_entries_are_served_at_their_paths_and_the_mirror_and_listings_agree() {
    let (_db, server, a) = site().await;
    let base = server.base();
    let g = post(base, &a, "guide", "Guides", None);
    let _i = post(base, &a, "guide", "Install", Some(&g));
    let (s, html) = get_public(base, "/guide/guides/install");
    assert_eq!(s, 200, "{html}");
    assert!(html.contains("Install"));
    let (s, md) = get_public(base, "/guide/guides/install.md");
    assert_eq!(s, 200);
    assert!(md.starts_with("# Install"), "{md}");
    let (s, llms) = get_public(base, "/llms.txt");
    assert_eq!(s, 200);
    assert!(llms.contains("/guide/guides/install.md"), "{llms}");
    let (_, sitemap) = get_public(base, "/sitemap.xml");
    assert!(sitemap.contains("/guide/guides/install"), "{sitemap}");
    let (s, _) = get_public(base, "/guide/install");
    assert_eq!(s, 404, "a nested entry is not at its bare slug");
}

#[tokio::test]
async fn the_archive_of_a_hierarchical_type_redirects_to_its_first_root_and_search_is_a_redirect() {
    let (_db, server, a) = site().await;
    let base = server.base();
    let first = post(base, &a, "guide", "Zeta first by position", None);
    let _second = post(base, &a, "guide", "Alpha", None);
    // Position decides, not the title: make "Zeta" first.
    call(
        "PUT",
        base,
        &a,
        &format!("/api/v1/posts/{first}"),
        Some(json!({"position": 1})),
    );
    let agent = ureq::AgentBuilder::new().redirects(0).build();
    let resp = match agent.get(&format!("{base}/guide")).call() {
        Ok(r) | Err(ureq::Error::Status(_, r)) => r,
        Err(e) => panic!("{e}"),
    };
    assert_eq!(resp.status(), 302);
    assert_eq!(
        resp.header("location"),
        Some("/guide/zeta-first-by-position")
    );
    let resp = match agent
        .get(&format!(
            "{base}/guide/search?q=hello+world&term=doc-version:v2"
        ))
        .call()
    {
        Ok(r) | Err(ureq::Error::Status(_, r)) => r,
        Err(e) => panic!("{e}"),
    };
    assert_eq!(resp.status(), 302);
    assert_eq!(
        resp.header("location"),
        Some("/search?s=hello%20world&type=guide&term=doc-version%3Av2")
    );
}

#[tokio::test]
async fn existing_urls_are_unchanged() {
    let db = TestDb::new().await;
    let admin = common::seed_user(db.pool(), Role::Admin).await;
    let server = TestServer::start(common::BIN, &db);
    let base = server.base();
    let a = common::login_cookie(base, &admin.email, &admin.password);
    create_type(base, &a, "note", "Note", "Notes");
    post(base, &a, "note", "A Note", None);
    let (_, p) = call(
        "POST",
        base,
        &a,
        "/api/v1/posts",
        Some(json!({"type": "post", "title": "Hello", "status": "published", "content": doc("h")})),
    );
    let _ = p;
    assert_eq!(get_public(base, "/note/a-note").0, 200);
    assert_eq!(
        get_public(base, "/note").0,
        200,
        "a flat type keeps its listing"
    );
    assert_eq!(get_public(base, "/post/hello").0, 200);
    assert_eq!(get_public(base, "/note/a-note.md").0, 200);
}
