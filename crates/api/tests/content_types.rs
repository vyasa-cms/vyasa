//! Content types and custom fields over HTTP (phase 99): managing them,
//! entries carrying values through REST and GraphQL, and every surface an
//! entry reaches — its public URL and archive, templates, bindings, search,
//! the Markdown mirror, `llms.txt`, export and import — plus the plugin
//! enable and boot rules for a slug an administrator's type holds.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::too_many_lines)]

mod common;

use std::io::Write as _;

use serde_json::{json, Value};
use sqlx::PgPool;
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

fn doc(words: &str) -> Value {
    json!({"schema_version": 1, "blocks": [{"kind": "paragraph", "attrs": {"text": words}}]})
}

async fn admin_and_editor(pool: &PgPool) -> (common::Seeded, common::Seeded) {
    (
        common::seed_user(pool, Role::Admin).await,
        common::seed_user(pool, Role::Editor).await,
    )
}

fn login(base: &str, who: &common::Seeded) -> String {
    common::login_cookie(base, &who.email, &who.password)
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

fn create_field(base: &str, cookie: &str, slug: &str, field: &Value) {
    let (status, body) = call(
        "POST",
        base,
        cookie,
        &format!("/api/v1/content-types/{slug}/fields"),
        Some(field.clone()),
    );
    assert_eq!(status, 201, "create field {field}: {body}");
}

/// Every kind of field on `product`.
fn product_fields(base: &str, cookie: &str) {
    for field in [
        json!({"key": "tagline", "label": "Tagline", "kind": "text"}),
        json!({"key": "notes", "label": "Notes", "kind": "textarea"}),
        json!({"key": "price", "label": "Price", "kind": "number", "required": true,
               "options": {"min": 0}}),
        json!({"key": "in_stock", "label": "In stock", "kind": "boolean"}),
        json!({"key": "launch", "label": "Launch", "kind": "date"}),
        json!({"key": "colour", "label": "Colour", "kind": "choice",
               "options": {"choices": ["red", "blue", "green"], "multiple": true}}),
        json!({"key": "link", "label": "Link", "kind": "url"}),
        json!({"key": "cover", "label": "Cover", "kind": "media"}),
        json!({"key": "related", "label": "Related", "kind": "entry"}),
        json!({"key": "secret", "label": "Secret", "kind": "entry"}),
    ] {
        create_field(base, cookie, "product", &field);
    }
}

async fn seed_media(pool: &PgPool, id: i64, owner: i64) {
    sqlx::query(
        "INSERT INTO media (id, owner_id, file_name, mime, byte_size, path, alt)
         VALUES ($1, $2, 'cover.png', 'image/png', 10, 'x/cover.png', 'A cover')",
    )
    .bind(id)
    .bind(owner)
    .execute(pool)
    .await
    .expect("media");
}

// ------------------------------------------------------------ management

#[tokio::test]
async fn managing_types_and_fields_takes_manage_options() {
    let db = TestDb::new().await;
    let (admin, editor) = admin_and_editor(db.pool()).await;
    let server = TestServer::start(common::BIN, &db);
    let base = server.base();
    let a = login(base, &admin);
    let e = login(base, &editor);

    // An editor may not manage types or fields...
    let (status, _) = call(
        "POST",
        base,
        &e,
        "/api/v1/content-types",
        Some(json!({"slug": "event", "singular": "Event", "plural": "Events"})),
    );
    assert_eq!(status, 403);
    // ...an administrator may.
    create_type(base, &a, "event", "Event", "Events");
    for (method, path, body) in [
        (
            "PUT",
            "/api/v1/content-types/event",
            Some(json!({"plural": "Happenings"})),
        ),
        ("DELETE", "/api/v1/content-types/event", None),
        (
            "POST",
            "/api/v1/content-types/event/fields",
            Some(json!({"key": "venue", "label": "Venue", "kind": "text"})),
        ),
        ("GET", "/api/v1/content-types/event/orphans", None),
    ] {
        let (status, body) = call(method, base, &e, path, body);
        assert_eq!(status, 403, "editor {method} {path}: {body}");
    }
    create_field(
        base,
        &a,
        "event",
        &json!({"key": "venue", "label": "Venue", "kind": "text"}),
    );
    for (method, path, body) in [
        (
            "PUT",
            "/api/v1/content-types/event/fields/venue",
            Some(json!({"label": "Place"})),
        ),
        ("DELETE", "/api/v1/content-types/event/fields/venue", None),
    ] {
        let (status, _) = call(method, base, &e, path, body);
        assert_eq!(status, 403, "editor {method} {path}");
    }

    // Anyone who edits entries reads the list and a type's fields.
    let (status, list) = call("GET", base, &e, "/api/v1/content-types", None);
    assert_eq!(status, 200);
    let event = list
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["slug"] == "event")
        .expect("listed");
    assert_eq!(event["owner"], "admin");
    assert_eq!(event["plugin"], false);
    assert!(list
        .as_array()
        .unwrap()
        .iter()
        .any(|t| t["slug"] == "post" && t["owner"] == "builtin"));
    let (status, fields) = call("GET", base, &e, "/api/v1/content-types/event/fields", None);
    assert_eq!(status, 200);
    assert_eq!(fields[0]["key"], "venue");
    assert_eq!(fields[0]["kind"], "text");
    let (status, fields) = call("GET", base, &e, "/api/v1/content-types/post/fields", None);
    assert_eq!((status, fields), (200, json!([])));

    // Relabel, never re-slug.
    let (status, body) = call(
        "PUT",
        base,
        &a,
        "/api/v1/content-types/event",
        Some(json!({"plural": "Happenings"})),
    );
    assert_eq!((status, &body["plural"]), (200, &json!("Happenings")));
    let (status, body) = call(
        "PUT",
        base,
        &a,
        "/api/v1/content-types/event",
        Some(json!({"slug": "events"})),
    );
    assert_eq!(status, 400, "{body}");
    assert!(body["message"].as_str().unwrap().contains("slug"), "{body}");

    // Slug rules: reserved, built-in, malformed, too long, taken.
    for (slug, want) in [
        ("feed", 400),
        ("admin", 400),
        ("post", 400),
        ("Bad", 400),
        ("a-slug-that-is-much-longer-than-32", 400),
        ("event", 409),
    ] {
        let (status, body) = call(
            "POST",
            base,
            &a,
            "/api/v1/content-types",
            Some(json!({"slug": slug, "singular": "X", "plural": "Xs"})),
        );
        assert_eq!(status, want, "{slug}: {body}");
    }
    // Built-in types are not changed here.
    let (status, body) = call(
        "PUT",
        base,
        &a,
        "/api/v1/content-types/post",
        Some(json!({"plural": "Articles"})),
    );
    assert_eq!(status, 400, "{body}");

    // A type with entries (any status) cannot be deleted; the count is said.
    let (status, entry) = call(
        "POST",
        base,
        &a,
        "/api/v1/posts",
        Some(json!({"type": "event", "title": "Gig", "content": doc("x")})),
    );
    assert_eq!(status, 201, "{entry}");
    let (status, body) = call("DELETE", base, &a, "/api/v1/content-types/event", None);
    assert_eq!(status, 409, "{body}");
    assert!(body["message"].as_str().unwrap().contains('1'), "{body}");
    let id = entry["id"].as_i64().unwrap();
    let (status, _) = call(
        "DELETE",
        base,
        &a,
        &format!("/api/v1/posts/{id}?force=true"),
        None,
    );
    assert_eq!(status, 204);
    let (status, _) = call("DELETE", base, &a, "/api/v1/content-types/event", None);
    assert_eq!(status, 204);
    // Gone at once: no restart.
    let (status, _) = call(
        "POST",
        base,
        &a,
        "/api/v1/posts",
        Some(json!({"type": "event", "title": "Gig", "content": doc("x")})),
    );
    assert_eq!(status, 400);
    let (_, list) = call("GET", base, &a, "/api/v1/content-types", None);
    assert!(!list
        .as_array()
        .unwrap()
        .iter()
        .any(|t| t["slug"] == "event"));
}

// ---------------------------------------------------------------- values

#[tokio::test]
async fn entries_carry_every_kind_of_field_through_rest_and_graphql() {
    let db = TestDb::new().await;
    let (admin, _editor) = admin_and_editor(db.pool()).await;
    seed_media(db.pool(), 99_001, admin.id).await;
    let server = TestServer::start(common::BIN, &db);
    let base = server.base();
    let a = login(base, &admin);
    create_type(base, &a, "product", "Product", "Products");
    product_fields(base, &a);

    let (status, other) = call(
        "POST",
        base,
        &a,
        "/api/v1/posts",
        Some(
            json!({"type": "page", "title": "Spec sheet", "content": doc("x"),
                    "status": "published"}),
        ),
    );
    assert_eq!(status, 201, "{other}");
    let other_id = other["id"].as_i64().unwrap();

    let values = json!({
        "tagline": "Tough and light",
        "notes": "Line one\nLine two",
        "price": 19.5,
        "in_stock": true,
        "launch": "2026-10-01",
        "colour": ["red", "blue"],
        "link": "https://example.com/p",
        "cover": 99_001,
        "related": other_id.to_string(),
    });
    // A draft does not need the required price...
    let (status, draft) = call(
        "POST",
        base,
        &a,
        "/api/v1/posts",
        Some(
            json!({"type": "product", "title": "Draft", "content": doc("x"),
                    "fields": {"tagline": "Soon"}}),
        ),
    );
    assert_eq!(status, 201, "{draft}");
    // ...publishing does, and says which field.
    let (status, body) = call(
        "POST",
        base,
        &a,
        "/api/v1/posts",
        Some(
            json!({"type": "product", "title": "Nope", "content": doc("x"),
                    "status": "published", "fields": {"tagline": "x"}}),
        ),
    );
    assert_eq!(status, 400, "{body}");
    assert!(
        body["message"]
            .as_str()
            .unwrap()
            .contains("fields.price: Price is required to publish"),
        "{body}"
    );
    let draft_id = draft["id"].as_i64().unwrap();
    let (status, body) = call(
        "PUT",
        base,
        &a,
        &format!("/api/v1/posts/{draft_id}"),
        Some(json!({"status": "published"})),
    );
    assert_eq!(status, 400, "{body}");

    // Every kind round-trips; ids come back as strings.
    let (status, created) = call(
        "POST",
        base,
        &a,
        "/api/v1/posts",
        Some(
            json!({"type": "product", "title": "Widget", "content": doc("x"),
                    "status": "published", "fields": values.clone()}),
        ),
    );
    assert_eq!(status, 201, "{created}");
    let id = created["id"].as_i64().unwrap();
    let fields = &created["fields"];
    assert_eq!(fields["tagline"], "Tough and light");
    assert_eq!(fields["notes"], "Line one\nLine two");
    assert_eq!(fields["price"], 19.5);
    assert_eq!(fields["in_stock"], true);
    assert_eq!(fields["launch"], "2026-10-01");
    assert_eq!(fields["colour"], json!(["red", "blue"]));
    assert_eq!(fields["link"], "https://example.com/p");
    assert_eq!(fields["cover"], "99001");
    assert_eq!(fields["related"], other_id.to_string());
    assert!(created["meta"].get("fields").is_none(), "{created}");
    assert_eq!(created["fields_missing"], json!([]));
    let (_, fetched) = call("GET", base, &a, &format!("/api/v1/posts/{id}"), None);
    assert_eq!(fetched["fields"], created["fields"]);
    let (_, listed) = call("GET", base, &a, "/api/v1/posts?type=product", None);
    assert!(listed["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p["fields"]["price"] == 19.5));

    // Validation: every problem, by field.
    let (status, body) = call(
        "PUT",
        base,
        &a,
        &format!("/api/v1/posts/{id}"),
        Some(json!({"fields": {
            "link": "javascript:alert(1)", "launch": "2026-02-30", "colour": ["pink"],
            "price": -1, "nope": 1, "cover": 424_242, "tagline": "two\nlines"
        }})),
    );
    assert_eq!(status, 400, "{body}");
    let message = body["message"].as_str().unwrap();
    for key in [
        "fields.link",
        "fields.launch",
        "fields.colour",
        "fields.price",
        "fields.nope",
        "fields.cover",
        "fields.tagline",
    ] {
        assert!(message.contains(key), "{key} in {message}");
    }
    // A meta echo never writes values.
    let (status, body) = call(
        "PUT",
        base,
        &a,
        &format!("/api/v1/posts/{id}"),
        Some(json!({"meta": {"fields": {"link": "javascript:alert(1)"}, "seo_title": "S"}})),
    );
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["fields"]["link"], "https://example.com/p");

    // GraphQL reads and writes the same values.
    let gql = |query: String| -> Value {
        let resp = http(
            ureq::post(&format!("{base}/api/graphql"))
                .set("cookie", &a)
                .send_json(json!({ "query": query })),
        );
        assert_eq!(resp.status(), 200);
        json_of(resp)
    };
    let read = gql(format!(
        "{{ post(id: {id}) {{ fields fieldsMissing meta }} }}"
    ));
    assert!(read["errors"].is_null(), "{read}");
    assert_eq!(read["data"]["post"]["fields"], created["fields"]);
    assert_eq!(read["data"]["post"]["fieldsMissing"], json!([]));
    assert!(read["data"]["post"]["meta"].get("fields").is_none());
    let wrote = gql(format!(
        "mutation {{ updatePost(id: {id}, input: {{ fields: {{tagline: \"From GraphQL\", price: 21}} }}) {{ fields }} }}"
    ));
    assert!(wrote["errors"].is_null(), "{wrote}");
    assert_eq!(
        wrote["data"]["updatePost"]["fields"]["tagline"],
        "From GraphQL"
    );
    assert_eq!(wrote["data"]["updatePost"]["fields"]["price"], 21);
    let refused = gql(format!(
        "mutation {{ updatePost(id: {id}, input: {{ fields: {{link: \"javascript:x\"}} }}) {{ id }} }}"
    ));
    assert!(
        refused["errors"].to_string().contains("fields.link"),
        "{refused}"
    );

    // Autosave and working copies carry the editor's values, checked.
    let (status, body) = call(
        "PUT",
        base,
        &a,
        &format!("/api/v1/posts/{id}/autosave"),
        Some(json!({"title": "Widget", "content": doc("x"),
                    "fields": {"tagline": "Autosaved", "price": 5}})),
    );
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["fields"]["tagline"], "Autosaved");
    let (status, body) = call(
        "PUT",
        base,
        &a,
        &format!("/api/v1/posts/{id}/autosave"),
        Some(json!({"title": "Widget", "content": doc("x"),
                    "fields": {"link": "javascript:x"}})),
    );
    assert_eq!(status, 400, "{body}");
    let (status, saved) = call(
        "POST",
        base,
        &a,
        &format!("/api/v1/posts/{id}/revisions"),
        Some(json!({"title": "Widget", "content": doc("y"),
                    "fields": {"tagline": "Working copy", "price": 7}})),
    );
    assert_eq!(status, 200, "{saved}");
    assert_eq!(saved["fields"]["tagline"], "Working copy");
    let (_, live) = call("GET", base, &a, &format!("/api/v1/posts/{id}"), None);
    assert_eq!(
        live["fields"]["tagline"], "From GraphQL",
        "working copy is not live"
    );
    // Restoring it brings the values back.
    let rid = saved["id"].as_i64().unwrap();
    let (status, restored) = call(
        "POST",
        base,
        &a,
        &format!("/api/v1/posts/{id}/revisions/{rid}/restore"),
        None,
    );
    assert_eq!(status, 200, "{restored}");
    assert_eq!(restored["fields"]["tagline"], "Working copy");

    // Duplicating copies the values.
    let (status, copy) = call(
        "POST",
        base,
        &a,
        &format!("/api/v1/posts/{id}/duplicate"),
        None,
    );
    assert_eq!(status, 201, "{copy}");
    assert_eq!(copy["status"], "draft");
    // (`fields` replaces the whole set, so the restored working copy holds
    // just the two values it was saved with.)
    assert_eq!(
        copy["fields"],
        json!({"tagline": "Working copy", "price": 7})
    );
    assert_eq!(copy["fields"], restored["fields"]);

    // Review focus 3: a deleted media item stays in `fields` as its id,
    // marked missing; saving the entry unchanged still works.
    let (status, body) = call(
        "PUT",
        base,
        &a,
        &format!("/api/v1/posts/{id}"),
        Some(json!({ "fields": values })),
    );
    assert_eq!(status, 200, "{body}");
    sqlx::query("DELETE FROM media WHERE id = 99001")
        .execute(db.pool())
        .await
        .unwrap();
    let (_, after) = call("GET", base, &a, &format!("/api/v1/posts/{id}"), None);
    assert_eq!(after["fields"]["cover"], "99001");
    assert_eq!(after["fields_missing"], json!(["cover"]));
    let (status, body) = call(
        "PUT",
        base,
        &a,
        &format!("/api/v1/posts/{id}"),
        Some(json!({"fields": after["fields"].clone(), "title": "Widget 2"})),
    );
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["fields_missing"], json!(["cover"]));
    // fieldsMissing is for those who may edit the entry: anonymous gets
    // null, not an oracle on what exists.
    let anon = json_of(http(ureq::post(&format!("{base}/api/graphql")).send_json(
        json!({
            "query": format!("{{ post(id: {id}) {{ fields fieldsMissing }} }}")
        }),
    )));
    assert!(anon["errors"].is_null(), "{anon}");
    assert!(anon["data"]["post"]["fieldsMissing"].is_null(), "{anon}");
    assert_eq!(anon["data"]["post"]["fields"]["cover"], "99001");
    // Trash and restore answer with the same marker.
    let (status, body) = call("DELETE", base, &a, &format!("/api/v1/posts/{id}"), None);
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["fields_missing"], json!(["cover"]), "{body}");
    let (status, body) = call(
        "POST",
        base,
        &a,
        &format!("/api/v1/posts/{id}/restore"),
        None,
    );
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["fields_missing"], json!(["cover"]), "{body}");

    // Kind change with stored values: refused with the count.
    let (status, body) = call(
        "PUT",
        base,
        &a,
        "/api/v1/content-types/product/fields/tagline",
        Some(json!({"kind": "textarea"})),
    );
    assert_eq!(status, 409, "{body}");
    // Deleting a field keeps the values (not served), until a clean-up.
    let (status, _) = call(
        "DELETE",
        base,
        &a,
        "/api/v1/content-types/product/fields/notes",
        None,
    );
    assert_eq!(status, 204);
    let (_, after) = call("GET", base, &a, &format!("/api/v1/posts/{id}"), None);
    assert!(after["fields"].get("notes").is_none(), "{after}");
    // A save that sends the defined fields (the editor's whole set) keeps
    // the deleted field's value: only the clean-up removes it.
    let (status, body) = call(
        "PUT",
        base,
        &a,
        &format!("/api/v1/posts/{id}"),
        Some(json!({"fields": after["fields"].clone()})),
    );
    assert_eq!(status, 200, "{body}");
    let (status, body) = call(
        "PUT",
        base,
        &a,
        &format!("/api/v1/posts/{id}/autosave"),
        Some(json!({"title": "W", "content": doc("x"), "fields": after["fields"].clone()})),
    );
    assert_eq!(status, 200, "{body}");
    // The autosave keeps the stored orphan (listed below) but serves
    // only defined fields.
    assert!(body["fields"].get("notes").is_none(), "{body}");
    let (status, orphans) = call(
        "GET",
        base,
        &a,
        "/api/v1/content-types/product/orphans",
        None,
    );
    assert_eq!(status, 200);
    assert_eq!(orphans, json!([{"key": "notes", "entries": 1}]));
    // The key cannot be reused until then.
    let (status, body) = call(
        "POST",
        base,
        &a,
        "/api/v1/content-types/product/fields",
        Some(json!({"key": "notes", "label": "Notes", "kind": "number"})),
    );
    assert_eq!(status, 409, "{body}");
    let (status, cleaned) = call(
        "POST",
        base,
        &a,
        "/api/v1/content-types/product/orphans/notes/clean-up",
        None,
    );
    assert_eq!(status, 200, "{cleaned}");
    assert_eq!(cleaned["entries"], 1);
    create_field(
        base,
        &a,
        "product",
        &json!({"key": "notes", "label": "Notes", "kind": "number"}),
    );
    // Reorder.
    let (_, fields) = call(
        "GET",
        base,
        &a,
        "/api/v1/content-types/product/fields",
        None,
    );
    let mut keys: Vec<String> = fields
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["key"].as_str().unwrap().to_owned())
        .collect();
    keys.reverse();
    let (status, ordered) = call(
        "PUT",
        base,
        &a,
        "/api/v1/content-types/product/field-order",
        Some(json!({ "keys": keys })),
    );
    assert_eq!(status, 200, "{ordered}");
    assert_eq!(ordered[0]["key"], json!(keys[0]));
}

// ------------------------------------------------------------- rendering

/// A theme with per-type templates for `product`.
fn theme_package() -> Vec<u8> {
    let manifest = "name = \"fieldtest\"\nversion = 1\nauthor = \"t\"\nrequired_api = 1\n";
    let tokens = r##"{"version":1,"colors":{"primary":{"light":"#123456"}}}"##;
    let layout = r#"{
      "index":[{"id":"posts","kind":"latest-posts"}],
      "single":[{"id":"body","kind":"post-content"}],
      "archive":[{"id":"list","kind":"latest-posts"}],
      "page":[{"id":"body","kind":"post-content"}],
      "search":[{"id":"find","kind":"search-box"}],
      "not-found":[{"id":"nf","kind":"content"}]}"#;
    let single = r#"<html><body><h1>{{ entry.title }}</h1>
<p class="tagline">{{ entry.fields.tagline }}</p>
<p class="price">P={{ entry.fields.price }}</p>
<a class="link" href="{{ entry.fields.link }}">link</a>
<img class="cover" src="{{ entry.fields.cover.url }}">
<a class="related" href="{{ entry.fields.related.url }}">R={{ entry.fields.related.title }}</a>
<span class="secret">S={{ entry.fields.secret.title }}</span>
<span class="gone">G=[{{ entry.fields.gone }}]</span>
{{ sections | safe }}</body></html>"#;
    let archive = r"<html><body><ul>{% for p in posts %}<li>{{ p.title }}|{{ p.fields.price }}|{{ p.fields.gone }}</li>{% endfor %}</ul></body></html>";
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut w = zip::ZipWriter::new(&mut buf);
        for (name, data) in [
            ("manifest.toml", manifest.as_bytes()),
            ("tokens.json", tokens.as_bytes()),
            ("layout.json", layout.as_bytes()),
            ("templates/single-product.tera", single.as_bytes()),
            ("templates/archive-product.tera", archive.as_bytes()),
        ] {
            w.start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            w.write_all(data).unwrap();
        }
        w.finish().unwrap();
    }
    buf.into_inner()
}

fn install_theme(base: &str, cookie: &str) {
    let bytes = theme_package();
    let boundary = "----vyasafields";
    let mut body = Vec::new();
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; \
             filename=\"t.vytheme\"\r\nContent-Type: application/zip\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(&bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    let resp = http(
        ureq::post(&format!("{base}/api/v1/themes"))
            .set("cookie", cookie)
            .set(
                "content-type",
                &format!("multipart/form-data; boundary={boundary}"),
            )
            .send_bytes(&body),
    );
    let status = resp.status();
    assert_eq!(status, 201, "theme install: {}", text(resp));
    let (_, themes) = call("GET", base, cookie, "/api/v1/themes", None);
    let id = themes
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "fieldtest")
        .map(|t| t["id"].as_i64().unwrap())
        .expect("installed");
    let (status, _) = call(
        "POST",
        base,
        cookie,
        &format!("/api/v1/themes/{id}/activate"),
        None,
    );
    assert_eq!(status, 200);
}

fn publish(base: &str, cookie: &str, body: Value) -> Value {
    let (status, created) = call("POST", base, cookie, "/api/v1/posts", Some(body));
    assert_eq!(status, 201, "{created}");
    created
}

#[tokio::test]
async fn a_type_and_its_fields_reach_every_public_surface() {
    let db = TestDb::new().await;
    let (admin, _editor) = admin_and_editor(db.pool()).await;
    seed_media(db.pool(), 99_002, admin.id).await;
    let server = TestServer::start(common::BIN, &db);
    let base = server.base();
    let a = login(base, &admin);
    install_theme(base, &a);
    create_type(base, &a, "product", "Product", "Products");
    product_fields(base, &a);

    let related = publish(
        base,
        &a,
        json!({"type": "page", "title": "Related page", "content": doc("r"),
               "status": "published"}),
    );
    let secret = publish(
        base,
        &a,
        json!({"type": "page", "title": "Secret draft title", "content": doc("s")}),
    );
    let widget = publish(
        base,
        &a,
        json!({"type": "product", "title": "Widget", "slug": "widget", "content": doc("w"),
        "status": "published",
        "fields": {
            "tagline": "<script>alert(1)</script> zanzibarite",
            "price": 10, "colour": ["red"],
            "link": "https://example.com/w",
            "cover": "99002",
            "related": related["id"].as_i64().unwrap().to_string(),
            "secret": secret["id"].as_i64().unwrap().to_string(),
        }}),
    );
    publish(
        base,
        &a,
        json!({"type": "product", "title": "Gadget", "slug": "gadget", "content": doc("g"),
               "status": "published",
               "fields": {"price": 30, "colour": ["red", "blue"], "tagline": "Shiny"}}),
    );
    publish(
        base,
        &a,
        json!({"type": "product", "title": "Doohickey", "slug": "doohickey",
               "content": doc("d"), "status": "published",
               "fields": {"price": 20, "colour": ["blue"]}}),
    );
    // A password-protected product: never in a binding, whatever it holds.
    publish(
        base,
        &a,
        json!({"type": "product", "title": "Guarded", "slug": "guarded",
               "content": doc("g"), "status": "published", "password": "pw-gate-1",
               "fields": {"price": 99, "colour": ["red"]}}),
    );
    // References to a password-protected and to a private entry.
    let locked_page = publish(
        base,
        &a,
        json!({"type": "page", "title": "Locked page title", "content": doc("l"),
               "status": "published", "password": "pw-gate-2"}),
    );
    let private_page = publish(
        base,
        &a,
        json!({"type": "page", "title": "Private page title", "content": doc("p"),
               "status": "private"}),
    );
    for (slug, target) in [("locked", &locked_page), ("hidden", &private_page)] {
        publish(
            base,
            &a,
            json!({"type": "product", "title": slug, "slug": slug, "content": doc("x"),
                   "status": "published",
                   "fields": {"price": 1, "secret": target["id"].as_i64().unwrap().to_string()}}),
        );
        let (status, page) = get_public(base, &format!("/product/{slug}"));
        assert_eq!(status, 200, "{page}");
        assert!(!page.contains("Locked page title"), "{page}");
        assert!(!page.contains("Private page title"), "{page}");
        assert!(page.contains("S=</span>"), "{page}");
    }

    // The single URL, through the per-type template, autoescaped.
    let (status, page) = get_public(base, "/product/widget");
    assert_eq!(status, 200, "{page}");
    assert!(page.contains("<h1>Widget</h1>"), "{page}");
    assert!(
        !page.contains("<script>alert(1)"),
        "unescaped field:\n{page}"
    );
    assert!(page.contains("&lt;script&gt;alert(1)"), "{page}");
    assert!(page.contains("P=10"), "{page}");
    assert!(
        page.contains("href=\"https:&#x2F;&#x2F;example.com&#x2F;w\""),
        "{page}"
    );
    assert!(
        page.contains("&#x2F;api&#x2F;v1&#x2F;media&#x2F;99002&#x2F;raw"),
        "{page}"
    );
    assert!(page.contains("R=Related page"), "{page}");
    // A draft entry's title never reaches the public.
    assert!(!page.contains("Secret draft title"), "{page}");
    assert!(
        page.contains("S=<") || page.contains("S=\n") || page.contains("S=</span>"),
        "{page}"
    );
    // A field the template names but no type defines renders empty.
    assert!(page.contains("G=[]"), "{page}");

    // The archive, with the cards' fields.
    let (status, archive) = get_public(base, "/product");
    assert_eq!(status, 200, "{archive}");
    assert!(archive.contains("<li>Widget|10|</li>"), "{archive}");
    assert!(archive.contains("<li>Gadget|30|</li>"), "{archive}");

    // Stale references render nothing, and never an error page.
    sqlx::query("DELETE FROM media WHERE id = 99002")
        .execute(db.pool())
        .await
        .unwrap();
    let rid = related["id"].as_i64().unwrap();
    let (status, _) = call("DELETE", base, &a, &format!("/api/v1/posts/{rid}"), None);
    assert_eq!(status, 200);
    // A content change makes the cached page stale.
    let wid = widget["id"].as_i64().unwrap();
    let (status, _) = call(
        "PUT",
        base,
        &a,
        &format!("/api/v1/posts/{wid}"),
        Some(json!({"title": "Widget"})),
    );
    assert_eq!(status, 200);
    let (status, page) = get_public(base, "/product/widget?fresh=1");
    assert_eq!(status, 200, "{page}");
    assert!(
        !page.contains("99002"),
        "deleted media still linked:\n{page}"
    );
    assert!(
        !page.contains("Related page"),
        "trashed entry still shown:\n{page}"
    );
    let (_, api) = call("GET", base, &a, &format!("/api/v1/posts/{wid}"), None);
    let mut missing: Vec<String> = serde_json::from_value(api["fields_missing"].clone()).unwrap();
    missing.sort();
    assert_eq!(missing, ["cover", "related"], "{api}");

    // A binding sorted and filtered by fields: red products, priciest first.
    publish(
        base,
        &a,
        json!({"type": "page", "title": "Red things", "slug": "red-things",
               "content": doc("."), "status": "published",
               "layout": [{"id": "grid", "kind": "collection", "settings": {
                   "heading": "Red products",
                   "bind": {"source": "product", "sort": "field:price:desc",
                            "where": {"colour": "red"}}}}]}),
    );
    let (status, red) = get_public(base, "/red-things");
    assert_eq!(status, 200, "{red}");
    let gadget = red.find("Gadget").expect("Gadget listed");
    let widget_at = red.find("Widget").expect("Widget listed");
    assert!(gadget < widget_at, "price descending:\n{red}");
    assert!(
        !red.contains("Doohickey"),
        "blue only, filtered out:\n{red}"
    );
    assert!(
        !red.contains("Guarded"),
        "a protected entry is never bound:\n{red}"
    );
    // A binding on a field that does not exist renders empty, not an error.
    publish(
        base,
        &a,
        json!({"type": "page", "title": "Nothing", "slug": "nothing",
               "content": doc("."), "status": "published",
               "layout": [{"id": "grid", "kind": "collection", "settings": {
                   "heading": "Gone",
                   "bind": {"source": "product", "sort": "field:gone",
                            "where": {"gone": "x"}}}}]}),
    );
    let (status, nothing) = get_public(base, "/nothing");
    assert_eq!(status, 200, "{nothing}");
    assert!(!nothing.contains("Gadget"), "{nothing}");

    // The vocabulary: the type as a source, its fields, its templates.
    let (status, vocab) = call("GET", base, &a, "/api/v1/themes/vocabulary", None);
    assert_eq!(status, 200);
    assert!(vocab["sources"]
        .as_array()
        .unwrap()
        .iter()
        .any(|s| s["slug"] == "product" && s["owner"] == "admin"));
    assert!(vocab["source_fields"]["product"]
        .as_array()
        .unwrap()
        .iter()
        .any(|f| f["key"] == "price" && f["kind"] == "number"));
    assert!(vocab["type_templates"]
        .as_array()
        .unwrap()
        .iter()
        .any(|t| t == "single-product.html"));

    // The Markdown mirror and llms.txt list non-empty text fields.
    let (status, md) = get_public(base, "/product/gadget.md");
    assert_eq!(status, 200, "{md}");
    assert!(md.contains("**Tagline:** Shiny"), "{md}");
    let (status, llms) = get_public(base, "/llms.txt");
    assert_eq!(status, 200);
    assert!(llms.contains("/product/gadget.md"), "{llms}");
    assert!(llms.contains("  - Tagline: Shiny"), "{llms}");
    // The sitemap lists the archive and the entries.
    let (_, sitemap) = get_public(base, "/sitemap.xml");
    assert!(sitemap.contains("/product/gadget"), "{sitemap}");

    // Search finds an entry by a text field's words.
    let mut found = false;
    for _ in 0..50 {
        let (_, hits) = call("GET", base, &a, "/api/v1/search?q=zanzibarite", None);
        if hits["hits"].as_array().is_some_and(|h| !h.is_empty()) {
            found = hits.to_string().contains("widget");
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    assert!(found, "search by a text field's words");
}

// ------------------------------------------------------- export / import

#[tokio::test]
async fn export_then_import_round_trips_types_fields_and_values() {
    let source_db = TestDb::new().await;
    let (admin, _editor) = admin_and_editor(source_db.pool()).await;
    let source = TestServer::start(common::BIN, &source_db);
    let base = source.base();
    let a = login(base, &admin);
    create_type(base, &a, "product", "Product", "Products");
    for field in [
        json!({"key": "tagline", "label": "Tagline", "kind": "text"}),
        json!({"key": "price", "label": "Price", "kind": "number"}),
        json!({"key": "link", "label": "Link", "kind": "url"}),
        json!({"key": "related", "label": "Related", "kind": "entry"}),
    ] {
        create_field(base, &a, "product", &field);
    }
    create_field(
        base,
        &a,
        "post",
        &json!({"key": "subtitle", "label": "Subtitle", "kind": "text"}),
    );
    let first = publish(
        base,
        &a,
        json!({"type": "product", "title": "First", "slug": "first", "content": doc("1"),
               "status": "published", "fields": {"tagline": "One", "price": 1}}),
    );
    publish(
        base,
        &a,
        json!({"type": "product", "title": "Second", "slug": "second", "content": doc("2"),
               "status": "published",
               "fields": {"tagline": "Two", "price": 2, "link": "/first",
                          "related": first["id"].as_i64().unwrap().to_string()}}),
    );
    publish(
        base,
        &a,
        json!({"title": "A post", "slug": "a-post", "content": doc("p"),
               "status": "published", "fields": {"subtitle": "Sub"}}),
    );
    publish(
        base,
        &a,
        json!({"type": "product", "title": "Third", "slug": "third", "content": doc("3"),
               "status": "published"}),
    );
    let resp = http(
        ureq::get(&format!("{base}/api/v1/export?format=json"))
            .set("cookie", &a)
            .call(),
    );
    assert_eq!(resp.status(), 200);
    let mut archive = json_of(resp);
    assert_eq!(archive["content_types"][0]["slug"], "product");
    assert!(archive["content_fields"].as_array().unwrap().len() >= 5);
    let second = archive["posts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["slug"] == "second")
        .unwrap()
        .clone();
    assert_eq!(second["fields"]["tagline"], "Two");
    assert!(second["meta"].get("fields").is_none());
    // A forged archive: values smuggled through raw meta, and a value
    // the field refuses. Neither may land unchecked.
    if let Some(posts) = archive["posts"].as_array_mut() {
        for p in posts.iter_mut() {
            if p["slug"] == "first" {
                p["meta"]["fields"] = json!({"link": "javascript:alert(1)"});
                p["fields"]["link"] = json!("javascript:alert(2)");
            }
            if p["slug"] == "third" {
                p["meta"]["fields"] = json!({"link": "javascript:alert(3)"});
            }
        }
    }
    drop(source);

    let target_db = TestDb::new().await;
    let (target_admin, _) = admin_and_editor(target_db.pool()).await;
    let target = TestServer::start(common::BIN, &target_db);
    let base = target.base();
    let t = login(base, &target_admin);
    let (status, report) = call("POST", base, &t, "/api/v1/import", Some(archive));
    assert_eq!(status, 200, "{report}");
    assert_eq!(report["content_types"], 1, "{report}");
    assert_eq!(report["content_fields"], 5, "{report}");
    let warnings = report["warnings"].to_string();
    assert!(warnings.contains("meta.fields ignored"), "{warnings}");
    assert!(warnings.contains("fields.link"), "{warnings}");

    // The type is live at once; its fields and values arrived.
    let (_, list) = call("GET", base, &t, "/api/v1/content-types", None);
    assert!(list
        .as_array()
        .unwrap()
        .iter()
        .any(|x| x["slug"] == "product" && x["owner"] == "admin"));
    let (status, second) = call("GET", base, &t, "/api/v1/posts/slug/product/second", None);
    assert_eq!(status, 200, "{second}");
    assert_eq!(second["fields"]["tagline"], "Two");
    assert_eq!(second["fields"]["price"], 2);
    assert_eq!(second["fields"]["link"], "/first");
    let (_, first) = call("GET", base, &t, "/api/v1/posts/slug/product/first", None);
    // The reference points at the entry as it is numbered here.
    assert_eq!(
        second["fields"]["related"],
        first["id"].as_i64().unwrap().to_string()
    );
    assert_eq!(first["fields"]["tagline"], "One");
    assert!(first["fields"].get("link").is_none(), "{first}");
    let stored: Value = sqlx::query_scalar("SELECT meta FROM posts WHERE slug = 'first'")
        .fetch_one(target_db.pool())
        .await
        .unwrap();
    assert!(!stored.to_string().contains("javascript"), "{stored}");
    let stored: Value = sqlx::query_scalar("SELECT meta FROM posts WHERE slug = 'third'")
        .fetch_one(target_db.pool())
        .await
        .unwrap();
    assert!(stored.get("fields").is_none(), "{stored}");
    let (_, post) = call("GET", base, &t, "/api/v1/posts/slug/post/a-post", None);
    assert_eq!(post["fields"]["subtitle"], "Sub");
    let (status, page) = get_public(base, "/product/second");
    assert_eq!(status, 200, "{page}");
}

// ---------------------------------------------------------------- plugins

const BOOKSHELF_WASM: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../plugin-sdk/examples/bookshelf/bookshelf-component.wasm"
);

/// Installs the bookshelf example (it declares the post type `book` and
/// the taxonomy `shelf`), disabled unless `enabled`.
async fn install_bookshelf(pool: &PgPool, enabled: bool) -> i64 {
    let wasm = std::fs::read(BOOKSHELF_WASM).expect("bookshelf fixture");
    let repo = vyasa_db::repo::PluginsRepo::new(pool.clone());
    let row = repo
        .create(
            "bookshelf",
            "0.1.0",
            &wasm,
            &vyasa_plugins::host::sha256_hex(&wasm),
            &json!(["log:write", "db:read:posts", "kv:read", "kv:write"]),
        )
        .await
        .expect("install plugin");
    if enabled {
        repo.set_enabled(row.id, true).await.expect("enable");
    }
    row.id
}

fn plugin_row(base: &str, cookie: &str, id: i64) -> Value {
    let (_, list) = call("GET", base, cookie, "/api/v1/plugins", None);
    list.as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == id)
        .cloned()
        .expect("plugin listed")
}

#[tokio::test]
async fn enabling_a_plugin_that_declares_an_admin_types_slug_is_refused() {
    let db = TestDb::new().await;
    let (admin, _) = admin_and_editor(db.pool()).await;
    let plugin = install_bookshelf(db.pool(), false).await;
    let server = TestServer::start(common::BIN, &db);
    let base = server.base();
    let a = login(base, &admin);

    // A plugin taxonomy may not take an admin type's slug: refused, and
    // the post type it had already registered is withdrawn with it.
    create_type(base, &a, "shelf", "Shelf", "Shelves");
    let (status, body) = call(
        "POST",
        base,
        &a,
        &format!("/api/v1/plugins/{plugin}/enable"),
        None,
    );
    assert_eq!(status, 409, "{body}");
    assert!(
        body["message"].as_str().unwrap().contains("taxonomy"),
        "{body}"
    );
    assert!(
        body["message"].as_str().unwrap().contains("\"shelf\""),
        "{body}"
    );
    let (status, body) = call(
        "POST",
        base,
        &a,
        "/api/v1/posts",
        Some(json!({"type": "book", "title": "x", "content": doc("x")})),
    );
    assert_eq!(
        status, 400,
        "the refused plugin's type stays unregistered: {body}"
    );
    let (status, _) = call("DELETE", base, &a, "/api/v1/content-types/shelf", None);
    assert_eq!(status, 204);

    // A plugin post type may not take one either.
    create_type(base, &a, "book", "Book", "Books");
    let (status, body) = call(
        "POST",
        base,
        &a,
        &format!("/api/v1/plugins/{plugin}/enable"),
        None,
    );
    assert_eq!(status, 409, "{body}");
    let message = body["message"].as_str().unwrap();
    assert!(message.contains("\"book\""), "{message}");
    assert!(message.contains("administrator"), "{message}");
    // Nothing half-registered: off, no surface, and the reason recorded.
    let row = plugin_row(base, &a, plugin);
    assert_eq!(row["enabled"], false, "{row}");
    assert!(
        row["status_reason"].as_str().unwrap().contains("book"),
        "{row}"
    );
    let (_, surface) = call("GET", base, &a, "/api/v1/plugins/surface", None);
    assert_eq!(surface["postTypes"], json!([]), "{surface}");
    assert_eq!(surface["taxonomies"], json!([]), "{surface}");
    assert_eq!(surface["blocks"], json!([]), "{surface}");
    // The admin type still owns the slug.
    let (_, list) = call("GET", base, &a, "/api/v1/content-types", None);
    let book: Vec<&Value> = list
        .as_array()
        .unwrap()
        .iter()
        .filter(|t| t["slug"] == "book")
        .collect();
    assert_eq!(book.len(), 1);
    assert_eq!(book[0]["owner"], "admin");

    // Once the type is gone the plugin enables, and its type is live.
    let (status, _) = call("DELETE", base, &a, "/api/v1/content-types/book", None);
    assert_eq!(status, 204);
    let (status, body) = call(
        "POST",
        base,
        &a,
        &format!("/api/v1/plugins/{plugin}/enable"),
        None,
    );
    assert_eq!(status, 200, "{body}");
    let (status, body) = call(
        "POST",
        base,
        &a,
        "/api/v1/posts",
        Some(json!({"type": "book", "title": "x", "content": doc("x")})),
    );
    assert_eq!(status, 201, "{body}");
    // An administrator cannot take a slug an enabled plugin declares.
    let (status, body) = call(
        "POST",
        base,
        &a,
        "/api/v1/content-types",
        Some(json!({"slug": "book", "singular": "B", "plural": "Bs"})),
    );
    assert_eq!(status, 409, "{body}");
    // Disabling takes its type out of the registry.
    let (status, _) = call(
        "POST",
        base,
        &a,
        &format!("/api/v1/plugins/{plugin}/disable"),
        None,
    );
    assert_eq!(status, 200);
    let (status, body) = call(
        "POST",
        base,
        &a,
        "/api/v1/posts",
        Some(json!({"type": "book", "title": "y", "content": doc("y")})),
    );
    assert_eq!(
        status, 400,
        "a disabled plugin's type no longer parses: {body}"
    );
}

#[tokio::test]
async fn at_boot_an_admin_type_keeps_its_slug_and_the_plugin_is_degraded() {
    let db = TestDb::new().await;
    let (admin, _) = admin_and_editor(db.pool()).await;
    sqlx::query(
        "INSERT INTO content_types (slug, singular, plural, description, public, has_archive)
         VALUES ('book', 'Volume', 'Volumes', '', true, true)",
    )
    .execute(db.pool())
    .await
    .expect("admin type");
    let plugin = install_bookshelf(db.pool(), true).await;
    let server = TestServer::start(common::BIN, &db);
    let base = server.base();
    let a = login(base, &admin);

    let row = plugin_row(base, &a, plugin);
    assert_eq!(row["status"], "degraded", "{row}");
    assert!(
        row["status_reason"].as_str().unwrap().contains("book"),
        "{row}"
    );
    // The plugin still runs the rest of what it declares.
    let (_, surface) = call("GET", base, &a, "/api/v1/plugins/surface", None);
    assert_eq!(surface["postTypes"], json!([]), "{surface}");
    assert_eq!(surface["taxonomies"][0]["slug"], "shelf", "{surface}");
    // And the slug is the administrator's type.
    let (_, list) = call("GET", base, &a, "/api/v1/content-types", None);
    let book = list
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["slug"] == "book")
        .cloned()
        .unwrap();
    assert_eq!(book["owner"], "admin");
    assert_eq!(book["plural"], "Volumes");
    publish(
        base,
        &a,
        json!({"type": "book", "title": "Dune", "slug": "dune", "content": doc("d"),
               "status": "published"}),
    );
    let (status, page) = get_public(base, "/book/dune");
    assert_eq!(status, 200, "{page}");
    let (status, archive) = get_public(base, "/book");
    assert_eq!(status, 200);
    assert!(archive.contains("Volumes"), "{archive}");
    // Plugin health counts it.
    let (_, health) = call("GET", base, &a, "/api/v1/site-health", None);
    assert!(health.to_string().contains("degraded") || health.to_string().contains("plugin"));
}

// ------------------------------------------------------- fix round 1

/// Polls the REST search (as `cookie`) until `want` holds for whether
/// `term` has hits, or gives up after ~10 s.
fn search_settles(base: &str, cookie: &str, term: &str, want: bool) -> bool {
    for _ in 0..50 {
        let (_, hits) = call(
            "GET",
            base,
            cookie,
            &format!("/api/v1/search?q={term}"),
            None,
        );
        let found = hits["hits"].as_array().is_some_and(|h| !h.is_empty());
        if found == want {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    false
}

fn gql(base: &str, cookie: Option<&str>, query: &str) -> Value {
    let mut req = ureq::post(&format!("{base}/api/graphql"));
    if let Some(c) = cookie {
        req = req.set("cookie", c);
    }
    let resp = http(req.send_json(json!({ "query": query })));
    assert_eq!(resp.status(), 200);
    json_of(resp)
}

#[tokio::test]
async fn a_non_public_type_stays_private_on_every_surface() {
    let db = TestDb::new().await;
    let (admin, _editor) = admin_and_editor(db.pool()).await;
    let subscriber = common::seed_user(db.pool(), Role::Subscriber).await;
    let server = TestServer::start(common::BIN, &db);
    let base = server.base();
    let a = login(base, &admin);
    let sub = login(base, &subscriber);
    let (status, body) = call(
        "POST",
        base,
        &a,
        "/api/v1/content-types",
        Some(
            json!({"slug": "staff", "singular": "Staff member", "plural": "Staff",
                    "public": false}),
        ),
    );
    assert_eq!(status, 201, "{body}");
    create_field(
        base,
        &a,
        "staff",
        &json!({"key": "bio", "label": "Bio", "kind": "text"}),
    );
    sqlx::query(
        "INSERT INTO terms (id, taxonomy, name, slug) VALUES (86391, 'category', 'Crew', 'crew')",
    )
    .execute(db.pool())
    .await
    .expect("category");
    let staff = publish(
        base,
        &a,
        json!({"type": "staff", "title": "Quokkaword Person", "slug": "quokka",
               "content": doc("q"), "status": "published",
               "fields": {"bio": "numbatword"}, "term_ids": [86391]}),
    );
    // A public entry in the same category, so its archive has a page.
    publish(
        base,
        &a,
        json!({"type": "post", "title": "Crew news", "slug": "crew-news",
               "content": doc("n"), "status": "published", "term_ids": [86391]}),
    );
    let staff_id = staff["id"].as_i64().unwrap();

    // HTML search: neither the index nor the SQL fallback (by title).
    let (status, page) = get_public(base, "/search?s=Quokkaword");
    assert_eq!(status, 200);
    assert!(!page.contains("Quokkaword Person"), "{page}");
    // REST search, anonymous and as a subscriber.
    for cookie in ["", sub.as_str()] {
        let (_, hits) = call("GET", base, cookie, "/api/v1/search?q=numbatword", None);
        assert_eq!(hits["hits"], json!([]), "{hits}");
    }
    // GraphQL: listings, by id, by slug, search.
    for cookie in [None, Some(sub.as_str())] {
        let listed = gql(
            base,
            cookie,
            r#"{ posts(postType: "staff") { edges { node { title fields } } totalCount } }"#,
        );
        assert_eq!(listed["data"]["posts"]["edges"], json!([]), "{listed}");
        assert_eq!(listed["data"]["posts"]["totalCount"], 0, "{listed}");
        let all = gql(
            base,
            cookie,
            "{ posts(first: 100) { edges { node { title } } } }",
        );
        assert!(!all.to_string().contains("Quokkaword"), "{all}");
        let one = gql(
            base,
            cookie,
            &format!("{{ post(id: {staff_id}) {{ title }} }}"),
        );
        assert!(!one.to_string().contains("Quokkaword"), "{one}");
        let by_slug = gql(
            base,
            cookie,
            r#"{ postBySlug(postType: "staff", slug: "quokka") { title } }"#,
        );
        assert!(!by_slug.to_string().contains("Quokkaword"), "{by_slug}");
        let found = gql(
            base,
            cookie,
            r#"{ search(query: "numbatword") { post { title } } }"#,
        );
        assert!(!found.to_string().contains("Quokkaword"), "{found}");
    }
    // REST reads as a subscriber.
    let (status, _) = call(
        "GET",
        base,
        &sub,
        &format!("/api/v1/posts/{staff_id}"),
        None,
    );
    assert_eq!(status, 404);
    let (status, _) = call("GET", base, &sub, "/api/v1/posts/slug/staff/quokka", None);
    assert_eq!(status, 404);
    let (_, listed) = call("GET", base, &sub, "/api/v1/posts?type=staff", None);
    assert_eq!(listed["items"], json!([]), "{listed}");
    assert_eq!(listed["total"], 0, "{listed}");
    // An editor still sees it.
    let (status, _) = call("GET", base, &a, &format!("/api/v1/posts/{staff_id}"), None);
    assert_eq!(status, 200);
    // Bindings, the sitemap and llms.txt leave it out.
    publish(
        base,
        &a,
        json!({"type": "page", "title": "Team", "slug": "team", "content": doc("."),
               "status": "published",
               "layout": [{"id": "grid", "kind": "collection", "settings": {
                   "heading": "Our team", "bind": {"source": "staff"}}}]}),
    );
    let (status, team) = get_public(base, "/team");
    assert_eq!(status, 200);
    assert!(!team.contains("Quokkaword"), "{team}");
    let (_, sitemap) = get_public(base, "/sitemap.xml");
    assert!(!sitemap.contains("/staff/"), "{sitemap}");
    let (_, llms) = get_public(base, "/llms.txt");
    assert!(!llms.contains("Quokkaword"), "{llms}");
    // The feeds, the home listing and the category archive leave it out;
    // its own page and its Markdown mirror do not exist for a reader.
    for path in ["/feed.xml", "/atom.xml", "/", "/category/crew"] {
        let (status, page) = get_public(base, path);
        assert_eq!(status, 200, "{path}");
        assert!(!page.contains("Quokkaword"), "{path}: {page}");
    }
    let (_, archive) = get_public(base, "/category/crew");
    assert!(archive.contains("Crew news"), "{archive}");
    for path in ["/staff/quokka", "/staff/quokka.md", "/staff"] {
        let (status, page) = get_public(base, path);
        assert_eq!(status, 404, "{path}: {page}");
        assert!(!page.contains("Quokkaword"), "{path}: {page}");
    }

    // Flipping the type public makes its entries searchable at once...
    let (status, _) = call(
        "PUT",
        base,
        &a,
        "/api/v1/content-types/staff",
        Some(json!({"public": true})),
    );
    assert_eq!(status, 200);
    assert!(
        search_settles(base, "", "numbatword", true),
        "a newly public type is indexed"
    );
    // ...and back to private takes them out of the index (asked as an
    // editor, whom no query-time filter applies to).
    let (status, _) = call(
        "PUT",
        base,
        &a,
        "/api/v1/content-types/staff",
        Some(json!({"public": false})),
    );
    assert_eq!(status, 200);
    assert!(
        search_settles(base, &a, "numbatword", false),
        "a type made private leaves the index"
    );
}

#[tokio::test]
async fn field_text_leaves_search_with_its_field_or_its_entry() {
    let db = TestDb::new().await;
    let (admin, _editor) = admin_and_editor(db.pool()).await;
    let server = TestServer::start(common::BIN, &db);
    let base = server.base();
    let a = login(base, &admin);
    create_type(base, &a, "product", "Product", "Products");
    create_field(
        base,
        &a,
        "product",
        &json!({"key": "tagline", "label": "Tagline", "kind": "text"}),
    );
    create_field(
        base,
        &a,
        "product",
        &json!({"key": "motto", "label": "Motto", "kind": "text"}),
    );
    let one = publish(
        base,
        &a,
        json!({"type": "product", "title": "One", "content": doc("1"), "status": "published",
               "fields": {"tagline": "wombatword", "motto": "kakapoword"}}),
    );
    let two = publish(
        base,
        &a,
        json!({"type": "product", "title": "Two", "content": doc("2"), "status": "published",
               "fields": {"motto": "takaheword"}}),
    );
    assert!(search_settles(base, &a, "wombatword", true));
    assert!(search_settles(base, &a, "takaheword", true));
    // A deleted field's text is no longer found.
    let (status, _) = call(
        "DELETE",
        base,
        &a,
        "/api/v1/content-types/product/fields/tagline",
        None,
    );
    assert_eq!(status, 204);
    assert!(
        search_settles(base, &a, "wombatword", false),
        "deleted field"
    );
    // Unpublishing or trashing an entry takes its field text out.
    let id = one["id"].as_i64().unwrap();
    let (status, _) = call(
        "PUT",
        base,
        &a,
        &format!("/api/v1/posts/{id}"),
        Some(json!({"status": "draft"})),
    );
    assert_eq!(status, 200);
    assert!(search_settles(base, &a, "kakapoword", false), "unpublished");
    let id = two["id"].as_i64().unwrap();
    let (status, _) = call("DELETE", base, &a, &format!("/api/v1/posts/{id}"), None);
    assert_eq!(status, 200);
    assert!(search_settles(base, &a, "takaheword", false), "trashed");
}

#[tokio::test]
async fn other_peoples_entries_of_an_admin_type_follow_the_post_rules() {
    let db = TestDb::new().await;
    let (admin, _editor) = admin_and_editor(db.pool()).await;
    let author = common::seed_user(db.pool(), Role::Author).await;
    let contributor = common::seed_user(db.pool(), Role::Contributor).await;
    let server = TestServer::start(common::BIN, &db);
    let base = server.base();
    let a = login(base, &admin);
    create_type(base, &a, "product", "Product", "Products");
    create_field(
        base,
        &a,
        "product",
        &json!({"key": "tagline", "label": "Tagline", "kind": "text"}),
    );
    let draft = publish(
        base,
        &a,
        json!({"type": "product", "title": "Admin draft", "content": doc("d"),
               "fields": {"tagline": "secret plan"}}),
    );
    let live = publish(
        base,
        &a,
        json!({"type": "product", "title": "Admin live", "content": doc("l"),
               "status": "published", "fields": {"tagline": "public"}}),
    );
    let draft_id = draft["id"].as_i64().unwrap();
    let live_id = live["id"].as_i64().unwrap();
    for who in [&author, &contributor] {
        let c = login(base, who);
        // Someone else's draft: not readable, not editable.
        let (status, _) = call("GET", base, &c, &format!("/api/v1/posts/{draft_id}"), None);
        assert_eq!(status, 403);
        for id in [draft_id, live_id] {
            let (status, body) = call(
                "PUT",
                base,
                &c,
                &format!("/api/v1/posts/{id}"),
                Some(json!({"fields": {"tagline": "mine now"}})),
            );
            assert_eq!(status, 403, "{body}");
        }
        let read = gql(
            base,
            Some(&c),
            &format!("{{ post(id: {draft_id}) {{ title fields }} }}"),
        );
        assert!(!read.to_string().contains("secret plan"), "{read}");
        let wrote = gql(
            base,
            Some(&c),
            &format!(
                "mutation {{ updatePost(id: {live_id}, input: {{ fields: {{tagline: \"x\"}} }}) {{ id }} }}"
            ),
        );
        assert!(!wrote["errors"].is_null(), "{wrote}");
        // The published one reads, without the editor-only marker.
        let read = gql(
            base,
            Some(&c),
            &format!("{{ post(id: {live_id}) {{ fields fieldsMissing }} }}"),
        );
        assert_eq!(
            read["data"]["post"]["fields"]["tagline"], "public",
            "{read}"
        );
        assert!(read["data"]["post"]["fieldsMissing"].is_null(), "{read}");
    }
    let (_, after) = call("GET", base, &a, &format!("/api/v1/posts/{live_id}"), None);
    assert_eq!(after["fields"]["tagline"], "public");
}

#[tokio::test]
async fn a_media_field_does_not_carry_over_in_an_import() {
    let source_db = TestDb::new().await;
    let (admin, _editor) = admin_and_editor(source_db.pool()).await;
    seed_media(source_db.pool(), 99_005, admin.id).await;
    let source = TestServer::start(common::BIN, &source_db);
    let base = source.base();
    let a = login(base, &admin);
    create_type(base, &a, "product", "Product", "Products");
    create_field(
        base,
        &a,
        "product",
        &json!({"key": "cover", "label": "Cover", "kind": "media"}),
    );
    create_field(
        base,
        &a,
        "product",
        &json!({"key": "related", "label": "Related", "kind": "entry"}),
    );
    let outside = publish(
        base,
        &a,
        json!({"title": "Not exported", "content": doc("x"), "status": "published"}),
    );
    publish(
        base,
        &a,
        json!({"type": "product", "title": "Pic", "slug": "pic", "content": doc("p"),
               "status": "published",
               "fields": {"cover": "99005",
                          "related": outside["id"].as_i64().unwrap().to_string()}}),
    );
    let resp = http(
        ureq::get(&format!("{base}/api/v1/export?format=json"))
            .set("cookie", &a)
            .call(),
    );
    let mut archive = json_of(resp);
    // The referenced entry is not in the archive.
    if let Some(posts) = archive["posts"].as_array_mut() {
        posts.retain(|p| p["slug"] != "not-exported");
    }
    drop(source);

    let target_db = TestDb::new().await;
    let (target_admin, _) = admin_and_editor(target_db.pool()).await;
    // An unrelated media row with the same id must not be adopted.
    seed_media(target_db.pool(), 99_005, target_admin.id).await;
    let target = TestServer::start(common::BIN, &target_db);
    let base = target.base();
    let t = login(base, &target_admin);
    let (status, report) = call("POST", base, &t, "/api/v1/import", Some(archive));
    assert_eq!(status, 200, "{report}");
    let warnings = report["warnings"].to_string();
    assert!(
        warnings.contains("cover") && warnings.contains("media"),
        "{warnings}"
    );
    assert!(warnings.contains("related"), "{warnings}");
    let (_, pic) = call("GET", base, &t, "/api/v1/posts/slug/product/pic", None);
    assert_eq!(pic["fields"], json!({}), "{pic}");
}

#[tokio::test]
async fn saving_a_degraded_plugins_settings_keeps_it_enabled_and_degraded() {
    let db = TestDb::new().await;
    let (admin, _) = admin_and_editor(db.pool()).await;
    sqlx::query(
        "INSERT INTO content_types (slug, singular, plural, description, public, has_archive)
         VALUES ('book', 'Volume', 'Volumes', '', true, true)",
    )
    .execute(db.pool())
    .await
    .expect("admin type");
    let plugin = install_bookshelf(db.pool(), true).await;
    let server = TestServer::start(common::BIN, &db);
    let base = server.base();
    let a = login(base, &admin);
    assert_eq!(plugin_row(base, &a, plugin)["status"], "degraded");

    let (status, body) = call(
        "PUT",
        base,
        &a,
        &format!("/api/v1/plugins/settings/{plugin}/decorate"),
        Some(json!(true)),
    );
    assert_eq!(status, 204, "{body}");
    let row = plugin_row(base, &a, plugin);
    assert_eq!(row["enabled"], true, "{row}");
    assert_eq!(row["status"], "degraded", "{row}");
    // Its other declarations are still live; the admin type keeps `book`.
    let (_, surface) = call("GET", base, &a, "/api/v1/plugins/surface", None);
    assert_eq!(surface["taxonomies"][0]["slug"], "shelf", "{surface}");
    let (_, list) = call("GET", base, &a, "/api/v1/content-types", None);
    assert!(list
        .as_array()
        .unwrap()
        .iter()
        .any(|t| t["slug"] == "book" && t["owner"] == "admin"));
}

#[tokio::test]
async fn the_openapi_document_names_content_fields_apart_from_form_fields() {
    let db = TestDb::new().await;
    let server = TestServer::start(common::BIN, &db);
    let (status, doc) = get_public(server.base(), "/api/openapi.json");
    assert_eq!(status, 200);
    let doc: Value = serde_json::from_str(&doc).unwrap();
    let schemas = &doc["components"]["schemas"];
    for name in ["ContentField", "ContentFieldInput", "ContentFieldChanges"] {
        assert!(schemas.get(name).is_some(), "{name} missing");
    }
    // `Field` (if published) is the forms' field, not a content field.
    if let Some(field) = schemas.get("Field") {
        assert!(field["properties"].get("type_slug").is_none(), "{field}");
    }
    // Field values are an object of arbitrary JSON values.
    let fields = &schemas["PostResponse"]["properties"]["fields"];
    assert_eq!(fields["type"], "object", "{fields}");
    assert!(
        fields
            .get("additionalProperties")
            .is_some_and(|a| a != &json!(false)),
        "{fields}"
    );
}

// ------------------------------------------------------- fix round 2

fn staff_type(base: &str, a: &str) {
    let (status, body) = call(
        "POST",
        base,
        a,
        "/api/v1/content-types",
        Some(
            json!({"slug": "staff", "singular": "Staff member", "plural": "Staff",
                    "public": false}),
        ),
    );
    assert_eq!(status, 201, "{body}");
}

#[tokio::test]
async fn related_posts_show_only_what_the_public_may_open() {
    let db = TestDb::new().await;
    let (admin, _) = admin_and_editor(db.pool()).await;
    let server = TestServer::start(common::BIN, &db);
    let base = server.base();
    let a = login(base, &admin);
    staff_type(base, &a);
    let (status, _) = call(
        "PUT",
        base,
        &a,
        "/api/v1/options",
        Some(json!({"ai_related_posts": true})),
    );
    assert!(status < 300);
    let mk = |body: Value| publish(base, &a, body)["id"].as_i64().unwrap();
    let origin = mk(json!({"title": "Origin", "content": doc("o"), "status": "published"}));
    let open = mk(json!({"title": "Open neighbour", "content": doc("n"), "status": "published"}));
    let hidden = mk(
        json!({"type": "staff", "title": "Hidden neighbour", "content": doc("h"),
                           "status": "published"}),
    );
    let locked = mk(json!({"title": "Locked neighbour", "content": doc("l"),
                           "status": "published", "password": "pw-gate-3"}));
    for id in [origin, open, hidden, locked] {
        sqlx::query(
            "INSERT INTO post_embeddings (post_id, model, text_hash, dims, vector)
             VALUES ($1, 'm', 'h', 2, ARRAY[1.0, 0.5]::real[])",
        )
        .bind(id)
        .execute(db.pool())
        .await
        .unwrap();
    }
    let (status, related) = call(
        "GET",
        base,
        &a,
        &format!("/api/v1/posts/{origin}/related?limit=10"),
        None,
    );
    assert_eq!(status, 200, "{related}");
    let text = related.to_string();
    assert!(text.contains("Open neighbour"), "{text}");
    assert!(!text.contains("Hidden neighbour"), "{text}");
    assert!(!text.contains("Locked neighbour"), "{text}");
}

#[tokio::test]
async fn newsletter_and_indexnow_skip_non_public_types() {
    let db = TestDb::new().await;
    let (admin, _) = admin_and_editor(db.pool()).await;
    sqlx::query(
        "INSERT INTO subscribers (id, email, status, token, confirmed_at)
         VALUES (77001, 'reader@example.com', 'confirmed', 'tok-77001', now())",
    )
    .execute(db.pool())
    .await
    .expect("subscriber");
    let server = TestServer::start(common::BIN, &db);
    let base = server.base();
    let a = login(base, &admin);
    staff_type(base, &a);
    let (status, body) = call(
        "PUT",
        base,
        &a,
        "/api/v1/options",
        Some(json!({"newsletter_enabled": true, "site_url": "https://example.com"})),
    );
    assert!(status < 300, "{body}");
    publish(
        base,
        &a,
        json!({"type": "staff", "title": "Hushhush Staffer", "slug": "hushhush",
               "content": doc("s"), "status": "published"}),
    );
    publish(
        base,
        &a,
        json!({"title": "Openly Announced", "slug": "openly", "content": doc("p"),
               "status": "published"}),
    );
    // The public post is mailed and pinged (the control)...
    let mut payloads = String::new();
    for _ in 0..50 {
        let rows: Vec<String> = sqlx::query_scalar("SELECT payload::text FROM jobs")
            .fetch_all(db.pool())
            .await
            .unwrap();
        payloads = rows.join("\n");
        if payloads.contains("Openly Announced") && payloads.contains("/openly") {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    assert!(payloads.contains("Openly Announced"), "{payloads}");
    assert!(payloads.contains("/post/openly"), "{payloads}");
    // ...the non-public entry is neither.
    assert!(!payloads.contains("Hushhush"), "{payloads}");
    assert!(!payloads.contains("/staff/hushhush"), "{payloads}");
}

#[tokio::test]
async fn comments_on_a_non_public_type_answer_like_a_missing_entry() {
    let db = TestDb::new().await;
    let (admin, _) = admin_and_editor(db.pool()).await;
    let server = TestServer::start(common::BIN, &db);
    let base = server.base();
    let a = login(base, &admin);
    staff_type(base, &a);
    let staff = publish(
        base,
        &a,
        json!({"type": "staff", "title": "S", "content": doc("s"), "status": "published"}),
    )["id"]
        .as_i64()
        .unwrap();
    let missing = 9_999_999_i64;
    let comment = json!({"author_name": "V", "author_email": "v@example.com", "content": "hi"});
    let (s1, b1) = call(
        "POST",
        base,
        "",
        &format!("/api/v1/posts/{staff}/comments"),
        Some(comment.clone()),
    );
    let (s2, b2) = call(
        "POST",
        base,
        "",
        &format!("/api/v1/posts/{missing}/comments"),
        Some(comment),
    );
    assert_eq!(s1, 404, "{b1}");
    assert_eq!((s1, &b1["code"]), (s2, &b2["code"]), "{b1} vs {b2}");
    let (s1, b1) = call(
        "GET",
        base,
        "",
        &format!("/api/v1/posts/{staff}/comments"),
        None,
    );
    let (s2, b2) = call(
        "GET",
        base,
        "",
        &format!("/api/v1/posts/{missing}/comments"),
        None,
    );
    assert_eq!(s1, 404, "{b1}");
    assert_eq!((s1, &b1["code"]), (s2, &b2["code"]));
    // The HTML comment form, too.
    let form = |id: i64| {
        let resp = http(
            ureq::post(&format!("{base}/comment"))
                .set("content-type", "application/x-www-form-urlencoded")
                .send_string(&format!(
                    "post_id={id}&author_name=V&author_email=v%40example.com&content=hi"
                )),
        );
        resp.status()
    };
    assert_eq!(form(staff), 404);
    assert_eq!(form(staff), form(missing));
    let stored: i64 = sqlx::query_scalar("SELECT count(*) FROM comments")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(stored, 0);
}

#[tokio::test]
async fn translations_stay_within_one_type_and_hide_unreadable_members() {
    let db = TestDb::new().await;
    let (admin, _) = admin_and_editor(db.pool()).await;
    let subscriber = common::seed_user(db.pool(), Role::Subscriber).await;
    let server = TestServer::start(common::BIN, &db);
    let base = server.base();
    let a = login(base, &admin);
    let sub = login(base, &subscriber);
    staff_type(base, &a);
    let post = publish(
        base,
        &a,
        json!({"title": "English", "slug": "english", "content": doc("e"),
               "status": "published", "lang": "en"}),
    )["id"]
        .as_i64()
        .unwrap();
    // A cross-type translation link is refused before anything is written.
    let (status, body) = call(
        "POST",
        base,
        &a,
        "/api/v1/posts",
        Some(
            json!({"type": "staff", "title": "Deutsch", "content": doc("d"),
                    "lang": "de", "translation_of": post}),
        ),
    );
    assert_eq!(status, 400, "{body}");
    let (_, listed) = call("GET", base, &a, "/api/v1/posts?type=staff", None);
    assert_eq!(listed["total"], 0, "nothing created: {listed}");
    let staff = publish(
        base,
        &a,
        json!({"type": "staff", "title": "Geheim", "slug": "geheim", "content": doc("g"),
               "status": "published"}),
    )["id"]
        .as_i64()
        .unwrap();
    let (status, body) = call(
        "PUT",
        base,
        &a,
        &format!("/api/v1/posts/{staff}"),
        Some(json!({"translation_of": post})),
    );
    assert_eq!(status, 400, "{body}");
    let (status, body) = call(
        "PUT",
        base,
        &a,
        &format!("/api/v1/posts/{staff}/language"),
        Some(json!({"lang": "de", "link_slug": "english"})),
    );
    assert_eq!(status, 400, "{body}");

    // A mixed group from before the rule: its unreadable member is
    // invisible to a reader, in the response and in hreflang.
    sqlx::query("UPDATE posts SET translation_group = $1, lang = 'de' WHERE id = $2")
        .bind(post)
        .bind(staff)
        .execute(db.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE posts SET translation_group = $1 WHERE id = $1")
        .bind(post)
        .execute(db.pool())
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO post_translations (post_id, lang, group_id) VALUES ($1, 'en', $1), ($2, 'de', $1)",
    )
    .bind(post)
    .bind(staff)
    .execute(db.pool())
    .await
    .unwrap();
    let (status, body) = call("GET", base, &sub, &format!("/api/v1/posts/{post}"), None);
    assert_eq!(status, 200);
    assert!(
        !body["translations"].to_string().contains("Geheim"),
        "{body}"
    );
    let (status, body) = call(
        "GET",
        base,
        &sub,
        &format!("/api/v1/posts/{post}/language"),
        None,
    );
    assert_eq!(status, 200);
    assert!(!body.to_string().contains("Geheim"), "{body}");
    let (_, page) = get_public(base, "/post/english");
    assert!(!page.contains("geheim"), "{page}");
    // An editor still sees the whole group.
    let (_, body) = call("GET", base, &a, &format!("/api/v1/posts/{post}"), None);
    assert!(
        body["translations"].to_string().contains("Geheim"),
        "{body}"
    );
}

#[tokio::test]
async fn fields_missing_is_for_those_who_may_edit_the_entry() {
    let db = TestDb::new().await;
    let (admin, _) = admin_and_editor(db.pool()).await;
    let subscriber = common::seed_user(db.pool(), Role::Subscriber).await;
    seed_media(db.pool(), 99_011, admin.id).await;
    let server = TestServer::start(common::BIN, &db);
    let base = server.base();
    let a = login(base, &admin);
    let sub = login(base, &subscriber);
    create_type(base, &a, "product", "Product", "Products");
    create_field(
        base,
        &a,
        "product",
        &json!({"key": "cover", "label": "Cover", "kind": "media"}),
    );
    let id = publish(
        base,
        &a,
        json!({"type": "product", "title": "P", "slug": "p", "content": doc("p"),
               "status": "published", "fields": {"cover": "99011"}}),
    )["id"]
        .as_i64()
        .unwrap();
    sqlx::query("DELETE FROM media WHERE id = 99011")
        .execute(db.pool())
        .await
        .unwrap();
    let (_, mine) = call("GET", base, &a, &format!("/api/v1/posts/{id}"), None);
    assert_eq!(mine["fields_missing"], json!(["cover"]));
    for path in [
        format!("/api/v1/posts/{id}"),
        "/api/v1/posts/slug/product/p".to_owned(),
    ] {
        let (status, theirs) = call("GET", base, &sub, &path, None);
        assert_eq!(status, 200);
        assert!(theirs["fields_missing"].is_null(), "{theirs}");
        assert_eq!(theirs["fields"]["cover"], "99011");
    }
}

// ------------------------------------------------------- fix round 3

#[tokio::test]
async fn comment_endpoints_never_say_whether_a_hidden_entry_exists() {
    let db = TestDb::new().await;
    let (admin, _) = admin_and_editor(db.pool()).await;
    let server = TestServer::start(common::BIN, &db);
    let base = server.base();
    let a = login(base, &admin);
    staff_type(base, &a);
    let staff = publish(
        base,
        &a,
        json!({"type": "staff", "title": "S", "content": doc("s"), "status": "published"}),
    )["id"]
        .as_i64()
        .unwrap();
    let draft = publish(base, &a, json!({"title": "D", "content": doc("d")}))["id"]
        .as_i64()
        .unwrap();
    let missing = 9_999_998_i64;

    // REST, with an invalid body: validation must not run before the
    // entry is found, or the 400 tells a hidden entry from a missing one.
    let bad = json!({"author_name": "", "author_email": "nope", "content": ""});
    let answer = |id: i64| {
        let (status, body) = call(
            "POST",
            base,
            "",
            &format!("/api/v1/posts/{id}/comments"),
            Some(bad.clone()),
        );
        (status, body["code"].clone())
    };
    let gone = answer(missing);
    assert_eq!(gone.0, 404, "{gone:?}");
    assert_eq!(answer(staff), gone, "unreadable type");
    assert_eq!(answer(draft), gone, "draft");
    // A valid body on a draft: the same 404, not "comments are closed".
    let ok_body = json!({"author_name": "V", "author_email": "v@example.com", "content": "hi"});
    let (status, body) = call(
        "POST",
        base,
        "",
        &format!("/api/v1/posts/{draft}/comments"),
        Some(ok_body),
    );
    assert_eq!((status, body["code"].clone()), gone, "{body}");

    // GraphQL: submit and list answer a hidden entry exactly as a missing id.
    let gql_err = |query: String| -> String {
        let out = gql(base, None, &query);
        out["errors"][0]["message"]
            .as_str()
            .unwrap_or_default()
            .replace(&staff.to_string(), "ID")
            .replace(&draft.to_string(), "ID")
            .replace(&missing.to_string(), "ID")
    };
    let submit = |id: i64| {
        gql_err(format!(
            "mutation {{ submitComment(input: {{postId: {id}, authorName: \"V\", authorEmail: \"v@example.com\", content: \"hi\"}}) {{ id }} }}"
        ))
    };
    let list = |id: i64| gql_err(format!("{{ comments(postId: {id}) {{ totalCount }} }}"));
    let gone = submit(missing);
    assert!(!gone.is_empty());
    assert_eq!(submit(staff), gone);
    assert_eq!(submit(draft), gone);
    let gone = list(missing);
    assert!(!gone.is_empty());
    assert_eq!(list(staff), gone);
    assert_eq!(list(draft), gone);
    let stored: i64 = sqlx::query_scalar("SELECT count(*) FROM comments")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(stored, 0);
}

#[tokio::test]
async fn a_language_link_finds_an_entry_of_the_entrys_own_type() {
    let db = TestDb::new().await;
    let (admin, _) = admin_and_editor(db.pool()).await;
    let server = TestServer::start(common::BIN, &db);
    let base = server.base();
    let a = login(base, &admin);
    // A post and a page share the slug "about".
    publish(
        base,
        &a,
        json!({"title": "About (post)", "slug": "about", "content": doc("p"),
               "status": "published"}),
    );
    let page = publish(
        base,
        &a,
        json!({"type": "page", "title": "About", "slug": "about", "content": doc("p"),
               "status": "published"}),
    )["id"]
        .as_i64()
        .unwrap();
    let (status, _) = call(
        "PUT",
        base,
        &a,
        &format!("/api/v1/posts/{page}/language"),
        Some(json!({"lang": "en"})),
    );
    assert_eq!(status, 200);
    let german = publish(
        base,
        &a,
        json!({"type": "page", "title": "Über", "slug": "ueber", "content": doc("u"),
               "status": "published"}),
    )["id"]
        .as_i64()
        .unwrap();
    let (status, body) = call(
        "PUT",
        base,
        &a,
        &format!("/api/v1/posts/{german}/language"),
        Some(json!({"lang": "de", "link_slug": "about"})),
    );
    assert_eq!(status, 200, "{body}");
    assert!(body["group"].to_string().contains("\"About\""), "{body}");
}

#[tokio::test]
async fn revisions_serve_only_the_fields_the_type_still_defines() {
    let db = TestDb::new().await;
    let (admin, _) = admin_and_editor(db.pool()).await;
    let server = TestServer::start(common::BIN, &db);
    let base = server.base();
    let a = login(base, &admin);
    create_type(base, &a, "product", "Product", "Products");
    for key in ["kept", "gone"] {
        create_field(
            base,
            &a,
            "product",
            &json!({"key": key, "label": key, "kind": "text"}),
        );
    }
    let id = publish(
        base,
        &a,
        json!({"type": "product", "title": "P", "content": doc("p"),
               "fields": {"kept": "a", "gone": "b"}}),
    )["id"]
        .as_i64()
        .unwrap();
    let (status, saved) = call(
        "POST",
        base,
        &a,
        &format!("/api/v1/posts/{id}/revisions"),
        Some(json!({"title": "P2", "content": doc("q"), "fields": {"kept": "c", "gone": "d"}})),
    );
    assert_eq!(status, 200, "{saved}");
    let (status, _) = call(
        "DELETE",
        base,
        &a,
        "/api/v1/content-types/product/fields/gone",
        None,
    );
    assert_eq!(status, 204);
    let (status, list) = call(
        "GET",
        base,
        &a,
        &format!("/api/v1/posts/{id}/revisions"),
        None,
    );
    assert_eq!(status, 200);
    let revisions = list.as_array().unwrap();
    assert!(!revisions.is_empty());
    for rev in revisions {
        assert!(rev["fields"].get("gone").is_none(), "{rev}");
    }
    let rid = saved["id"].as_i64().unwrap();
    let (_, one) = call(
        "GET",
        base,
        &a,
        &format!("/api/v1/posts/{id}/revisions/{rid}"),
        None,
    );
    assert_eq!(one["fields"], json!({"kept": "c"}), "{one}");
    let (status, auto) = call(
        "PUT",
        base,
        &a,
        &format!("/api/v1/posts/{id}/autosave"),
        Some(json!({"title": "P", "content": doc("p")})),
    );
    assert_eq!(status, 200);
    assert!(auto["fields"].get("gone").is_none(), "{auto}");
    // Stored data unchanged: the orphan is still there to clean up.
    let (_, orphans) = call(
        "GET",
        base,
        &a,
        "/api/v1/content-types/product/orphans",
        None,
    );
    assert_eq!(orphans[0]["key"], "gone", "{orphans}");
}

/// Every post endpoint that answers with an entry serves the values of
/// the fields its type defines now — never a deleted field's stored
/// value — whichever path built the response.
#[tokio::test]
async fn every_entry_response_serves_only_the_fields_the_type_defines() {
    let db = TestDb::new().await;
    let (admin, _) = admin_and_editor(db.pool()).await;
    let server = TestServer::start(common::BIN, &db);
    let base = server.base();
    let a = login(base, &admin);
    create_type(base, &a, "gadget", "Gadget", "Gadgets");
    for key in ["kept", "gone"] {
        create_field(
            base,
            &a,
            "gadget",
            &json!({"key": key, "label": key, "kind": "text"}),
        );
    }
    let want = json!({"kept": "k"});
    let (status, created) = call(
        "POST",
        base,
        &a,
        "/api/v1/posts",
        Some(
            json!({"type": "gadget", "title": "G", "slug": "g", "content": doc("g"),
                    "status": "published", "fields": {"kept": "k", "gone": "x"}}),
        ),
    );
    assert_eq!(status, 201, "{created}");
    let id = created["id"].as_str().map_or_else(
        || created["id"].as_i64().unwrap().to_string(),
        str::to_owned,
    );
    let (status, _) = call(
        "DELETE",
        base,
        &a,
        "/api/v1/content-types/gadget/fields/gone",
        None,
    );
    assert_eq!(status, 204);

    let check = |what: &str, body: &Value| {
        assert_eq!(body["fields"], want, "{what}: {body}");
    };
    let (status, body) = call("GET", base, &a, &format!("/api/v1/posts/{id}"), None);
    assert_eq!(status, 200);
    check("single", &body);
    let (_, body) = call("GET", base, &a, "/api/v1/posts/slug/gadget/g", None);
    check("by slug", &body);
    let (_, body) = call("GET", base, &a, "/api/v1/posts?type=gadget", None);
    check("list", &body["items"][0]);
    let (status, body) = call(
        "PUT",
        base,
        &a,
        &format!("/api/v1/posts/{id}"),
        Some(json!({"title": "G2"})),
    );
    assert_eq!(status, 200, "{body}");
    check("update", &body);
    let (status, body) = call(
        "POST",
        base,
        &a,
        &format!("/api/v1/posts/{id}/duplicate"),
        None,
    );
    assert_eq!(status, 201, "{body}");
    check("duplicate", &body);
    let (status, body) = call("DELETE", base, &a, &format!("/api/v1/posts/{id}"), None);
    assert_eq!(status, 200, "{body}");
    check("trash", &body);
    let (status, body) = call(
        "POST",
        base,
        &a,
        &format!("/api/v1/posts/{id}/restore"),
        None,
    );
    assert_eq!(status, 200, "{body}");
    check("restore", &body);
    let (_, revs) = call(
        "GET",
        base,
        &a,
        &format!("/api/v1/posts/{id}/revisions"),
        None,
    );
    let rid = &revs[0]["id"];
    let rid = rid
        .as_str()
        .map_or_else(|| rid.as_i64().unwrap().to_string(), str::to_owned);
    let (status, body) = call(
        "POST",
        base,
        &a,
        &format!("/api/v1/posts/{id}/revisions/{rid}/restore"),
        None,
    );
    assert_eq!(status, 200, "{body}");
    check("revision restore", &body);
}
