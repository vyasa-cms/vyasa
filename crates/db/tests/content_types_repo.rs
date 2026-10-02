//! Content types and fields: the `content_types` and `content_fields`
//! tables, the per-field value queries over `posts.meta.fields`, and the
//! revision copy of those values.

#![allow(clippy::pedantic, clippy::unwrap_used, clippy::expect_used)]

use serde_json::json;
use vyasa_common::AppError;
use vyasa_db::content_models::{PostStatus, PostType};
use vyasa_db::models::Role;
use vyasa_db::repo::{
    ContentFieldUpdate, ContentFieldsRepo, ContentTypeUpdate, ContentTypesRepo, NewContentField,
    NewContentType, NewPost, NewRevision, NewUser, PostUpdate, PostsRepo, RevisionsRepo,
    TypeDeletion, UsersRepo,
};
use vyasa_testkit::TestDb;

async fn author(pool: &sqlx::PgPool) -> i64 {
    let id = vyasa_common::next_id_i64();
    UsersRepo::new(pool.clone())
        .insert(&NewUser {
            id,
            email: &format!("ct{id}@example.com"),
            username: &format!("ct{id}"),
            display_name: "CT",
            password_hash: Some("x"),
            role: Role::Admin,
            bio: "",
        })
        .await
        .expect("user");
    id
}

async fn entry(
    pool: &sqlx::PgPool,
    author_id: i64,
    post_type: &str,
    status: PostStatus,
    meta: serde_json::Value,
) -> i64 {
    let id = vyasa_common::next_id_i64();
    PostsRepo::new(pool.clone())
        .insert(&NewPost {
            id,
            post_type: PostType::from_db(post_type).expect("type"),
            status,
            slug: format!("e-{id}"),
            title: "Entry".into(),
            content: json!({"schema_version": 1, "blocks": []}),
            excerpt: None,
            author_id,
            parent_id: None,
            meta,
            published_at: None,
            scheduled_for: None,
            password_hash: None,
            layout: None,
        })
        .await
        .expect("entry");
    id
}

fn new_type(slug: &str) -> NewContentType<'_> {
    NewContentType {
        slug,
        singular: "Product",
        plural: "Products",
        description: "Things we sell",
        public: true,
        has_archive: false,
    }
}

fn new_field<'a>(type_slug: &'a str, key: &'a str, kind: &'a str) -> NewContentField<'a> {
    NewContentField {
        type_slug,
        key,
        label: "Label",
        help: "",
        kind,
        required: false,
        options: json!({}),
    }
}

#[tokio::test]
async fn content_types_are_inserted_listed_relabelled_and_deleted() {
    let db = TestDb::new().await;
    let types = ContentTypesRepo::new(db.pool().clone());

    let made = types.insert(&new_type("product")).await.expect("insert");
    assert_eq!(made.slug, "product");
    assert_eq!(made.singular, "Product");
    assert_eq!(made.plural, "Products");
    assert_eq!(made.description, "Things we sell");
    assert!(made.public);
    assert!(!made.has_archive);

    types.insert(&new_type("event")).await.expect("second");
    let listed: Vec<String> = types
        .list()
        .await
        .expect("list")
        .into_iter()
        .map(|t| t.slug)
        .collect();
    assert_eq!(listed, ["event", "product"], "listed by slug");

    let relabelled = types
        .update(
            "product",
            &ContentTypeUpdate {
                singular: Some("Item"),
                has_archive: Some(true),
                ..ContentTypeUpdate::default()
            },
        )
        .await
        .expect("update");
    assert_eq!(relabelled.singular, "Item");
    assert_eq!(relabelled.plural, "Products", "untouched");
    assert!(relabelled.has_archive);

    assert!(matches!(
        types.get("nope").await.unwrap_err(),
        AppError::NotFound { .. }
    ));
    assert!(matches!(
        types
            .update("nope", &ContentTypeUpdate::default())
            .await
            .unwrap_err(),
        AppError::NotFound { .. }
    ));

    assert_eq!(
        types.delete_if_unused("product").await.expect("delete"),
        TypeDeletion::Deleted
    );
    assert_eq!(
        types.delete_if_unused("product").await.expect("again"),
        TypeDeletion::Missing
    );
}

#[tokio::test]
async fn a_taken_slug_is_a_conflict_even_when_two_inserts_race() {
    let db = TestDb::new().await;
    let types = ContentTypesRepo::new(db.pool().clone());
    types.insert(&new_type("gadget")).await.expect("first");
    assert!(matches!(
        types.insert(&new_type("gadget")).await.unwrap_err(),
        AppError::Conflict { .. }
    ));

    // Two writers at once (the in-process lock aside): the primary key
    // lets exactly one through and the other is a conflict, not a 500.
    let a = ContentTypesRepo::new(db.pool().clone());
    let b = ContentTypesRepo::new(db.pool().clone());
    let (ta, tb) = (new_type("racer"), new_type("racer"));
    let (ra, rb) = tokio::join!(a.insert(&ta), b.insert(&tb));
    let oks = [ra.is_ok(), rb.is_ok()].iter().filter(|ok| **ok).count();
    assert_eq!(oks, 1, "exactly one insert wins: {ra:?} / {rb:?}");
    for r in [ra, rb] {
        if let Err(e) = r {
            assert!(matches!(e, AppError::Conflict { .. }), "{e}");
        }
    }
    let racers = types
        .list()
        .await
        .expect("list")
        .into_iter()
        .filter(|t| t.slug == "racer")
        .count();
    assert_eq!(racers, 1);
}

#[tokio::test]
async fn a_type_with_entries_in_any_status_is_not_deleted_and_the_count_is_reported() {
    let db = TestDb::new().await;
    let pool = db.pool().clone();
    let types = ContentTypesRepo::new(pool.clone());
    let fields = ContentFieldsRepo::new(pool.clone());
    types.insert(&new_type("listing")).await.expect("type");
    fields
        .insert(&new_field("listing", "price", "number"))
        .await
        .expect("field");
    let me = author(&pool).await;
    entry(&pool, me, "listing", PostStatus::Draft, json!({})).await;
    let trashed = entry(&pool, me, "listing", PostStatus::Trash, json!({})).await;

    assert_eq!(types.count_entries("listing").await.expect("count"), 2);
    assert_eq!(
        types.delete_if_unused("listing").await.expect("refused"),
        TypeDeletion::InUse(2),
        "a trashed entry counts too"
    );
    types.get("listing").await.expect("still there");

    // Once the entries are gone the type goes, and its fields with it.
    PostsRepo::new(pool.clone())
        .hard_delete(trashed)
        .await
        .expect("purge");
    sqlx::query("DELETE FROM posts WHERE type = 'listing'")
        .execute(&pool)
        .await
        .expect("purge rest");
    assert_eq!(
        types.delete_if_unused("listing").await.expect("delete"),
        TypeDeletion::Deleted
    );
    assert!(fields.list("listing").await.expect("list").is_empty());
}

#[tokio::test]
async fn fields_are_appended_in_order_reordered_updated_and_deleted() {
    let db = TestDb::new().await;
    let fields = ContentFieldsRepo::new(db.pool().clone());

    let first = fields
        .insert(&new_field("post", "subtitle", "text"))
        .await
        .expect("first");
    assert_eq!(first.position, 0);
    let second = fields
        .insert(&NewContentField {
            required: true,
            options: json!({"min": 0}),
            ..new_field("post", "rating", "number")
        })
        .await
        .expect("second");
    assert_eq!(second.position, 1, "appended after the last");
    assert!(second.required);
    assert_eq!(second.options, json!({"min": 0}));
    fields
        .insert(&new_field("page", "subtitle", "text"))
        .await
        .expect("same key on another type is fine");
    assert_eq!(fields.count("post").await.expect("count"), 2);

    assert!(matches!(
        fields
            .insert(&new_field("post", "subtitle", "url"))
            .await
            .unwrap_err(),
        AppError::Conflict { .. }
    ));

    let keys = |rows: Vec<vyasa_db::repo::ContentFieldRow>| -> Vec<String> {
        rows.into_iter().map(|f| f.key).collect()
    };
    assert_eq!(
        keys(fields.list("post").await.expect("list")),
        ["subtitle", "rating"]
    );
    let reordered = fields
        .reorder("post", &["rating".to_owned(), "subtitle".to_owned()])
        .await
        .expect("reorder");
    assert_eq!(keys(reordered), ["rating", "subtitle"]);
    assert_eq!(
        keys(fields.list("post").await.expect("list")),
        ["rating", "subtitle"]
    );

    let changed = fields
        .update(
            "post",
            "rating",
            &ContentFieldUpdate {
                label: Some("Stars"),
                kind: Some("text"),
                options: Some(&json!({"max_length": 5})),
                ..ContentFieldUpdate::default()
            },
        )
        .await
        .expect("update");
    assert_eq!(changed.label, "Stars");
    assert_eq!(changed.kind, "text");
    assert_eq!(changed.options, json!({"max_length": 5}));
    assert!(changed.required, "untouched");

    assert!(fields.delete("post", "rating").await.expect("delete"));
    assert!(!fields.delete("post", "rating").await.expect("gone"));
    assert!(matches!(
        fields.get("post", "rating").await.unwrap_err(),
        AppError::NotFound { .. }
    ));
}

#[tokio::test]
async fn stored_values_are_counted_per_key_and_cleared_from_entries_and_revisions() {
    let db = TestDb::new().await;
    let pool = db.pool().clone();
    let fields = ContentFieldsRepo::new(pool.clone());
    let me = author(&pool).await;
    let with = entry(
        &pool,
        me,
        "post",
        PostStatus::Published,
        json!({"seo": "keep", "fields": {"colour": "red", "size": "L"}}),
    )
    .await;
    entry(
        &pool,
        me,
        "post",
        PostStatus::Trash,
        json!({"fields": {"colour": "blue"}}),
    )
    .await;
    // Empty values do not count as storing anything.
    entry(
        &pool,
        me,
        "post",
        PostStatus::Draft,
        json!({"fields": {"colour": "", "size": null, "tags": []}}),
    )
    .await;
    // Another type's values are its own.
    entry(
        &pool,
        me,
        "page",
        PostStatus::Draft,
        json!({"fields": {"colour": "green"}}),
    )
    .await;
    let revisions = RevisionsRepo::new(pool.clone());
    let rev = revisions
        .insert(&NewRevision {
            id: vyasa_common::next_id_i64(),
            post_id: with,
            title: "Entry",
            content: json!({"schema_version": 1, "blocks": []}),
            author_id: me,
            is_autosave: false,
            layout: None,
            fields: Some(json!({"colour": "red", "size": "M"})),
        })
        .await
        .expect("revision");
    assert_eq!(rev.fields, Some(json!({"colour": "red", "size": "M"})));

    assert_eq!(
        fields
            .count_entries_with_value("post", "colour")
            .await
            .expect("count"),
        2,
        "the trashed entry counts; the empty one does not"
    );
    assert_eq!(
        fields
            .count_entries_with_value("post", "tags")
            .await
            .expect("count"),
        0
    );
    let mut counts = fields.stored_value_counts("post").await.expect("counts");
    counts.sort();
    assert_eq!(
        counts,
        [("colour".to_owned(), 2), ("size".to_owned(), 1)],
        "empty values are not counted"
    );

    let cleared = fields.clear_values("post", "colour").await.expect("clear");
    assert_eq!(cleared, 3, "the key goes wherever it is, empty values too");
    assert_eq!(
        fields
            .count_entries_with_value("post", "colour")
            .await
            .expect("count"),
        0
    );
    let kept = PostsRepo::new(pool.clone()).get(with).await.expect("entry");
    assert_eq!(
        kept.meta,
        json!({"seo": "keep", "fields": {"size": "L"}}),
        "only that key goes; the rest of meta stays"
    );
    let rev = revisions.get(rev.id).await.expect("revision");
    assert_eq!(
        rev.fields,
        Some(json!({"size": "M"})),
        "a revision restore must not bring a cleaned-up value back"
    );
    assert_eq!(
        fields
            .count_entries_with_value("page", "colour")
            .await
            .expect("other type"),
        1
    );
}

#[tokio::test]
async fn a_meta_write_keeps_stored_field_values_and_a_fields_write_keeps_the_rest_of_meta() {
    let db = TestDb::new().await;
    let pool = db.pool().clone();
    let posts = PostsRepo::new(pool.clone());
    let me = author(&pool).await;
    let id = entry(
        &pool,
        me,
        "post",
        PostStatus::Draft,
        json!({"seo": "old", "fields": {"colour": "red"}}),
    )
    .await;
    let update = |meta: Option<serde_json::Value>, fields: Option<serde_json::Value>| PostUpdate {
        id,
        status: None,
        slug: None,
        title: None,
        content: None,
        excerpt: None,
        parent_id: None,
        meta,
        fields,
        clear_schedule: false,
        scheduled_for: None,
        password_hash: None,
        term_ids: None,
        expect_status: None,
        touch_updated_at: false,
        layout: None,
    };

    // An SEO-only meta write must not wipe the values.
    let row = posts
        .update(&update(Some(json!({"seo": "new"})), None))
        .await
        .expect("meta");
    assert_eq!(row.meta, json!({"seo": "new", "fields": {"colour": "red"}}));

    // A `fields` key inside a plain meta write is not a way around the
    // values' own path: it is ignored and the stored values are kept.
    let row = posts
        .update(&update(
            Some(json!({"seo": "new", "fields": {"sneaky": 1}})),
            None,
        ))
        .await
        .expect("meta with fields");
    assert_eq!(row.meta, json!({"seo": "new", "fields": {"colour": "red"}}));

    // A fields write replaces the values and leaves the rest of meta.
    let row = posts
        .update(&update(None, Some(json!({"size": "L"}))))
        .await
        .expect("fields");
    assert_eq!(row.meta, json!({"seo": "new", "fields": {"size": "L"}}));

    // Both at once: the meta, with these values.
    let row = posts
        .update(&update(
            Some(json!({"seo": "both", "fields": {"ignored": 1}})),
            Some(json!({"colour": "blue"})),
        ))
        .await
        .expect("both");
    assert_eq!(
        row.meta,
        json!({"seo": "both", "fields": {"colour": "blue"}})
    );

    // An empty object clears them.
    let row = posts
        .update(&update(None, Some(json!({}))))
        .await
        .expect("clear");
    assert_eq!(row.meta, json!({"seo": "both"}));
}

/// Neighbours for related posts and semantic re-ranking: the public set
/// (`Some`) leaves out protected entries, blocks and types not listed;
/// an editor's set (`None`) keeps every type but blocks.
#[tokio::test]
async fn embeddings_are_listed_by_who_may_read_them() {
    let db = TestDb::new().await;
    let pool = db.pool().clone();
    let author_id = author(&pool).await;
    let mut ids = Vec::new();
    for (ty, protected) in [
        ("post", false),
        ("post", true),
        ("block", false),
        ("emb-hidden", false),
        ("emb-open", false),
    ] {
        let id = vyasa_common::next_id_i64();
        sqlx::query(
            "INSERT INTO posts (id, author_id, type, status, slug, title, content, meta, \
             password_hash, published_at)
             VALUES ($1, $2, $3, 'published', $4, 'T', '{}', '{}', $5, now())",
        )
        .bind(id)
        .bind(author_id)
        .bind(ty)
        .bind(format!("s{id}"))
        .bind(protected.then_some("hash"))
        .execute(&pool)
        .await
        .expect("post");
        sqlx::query(
            "INSERT INTO post_embeddings (post_id, model, text_hash, dims, vector)
             VALUES ($1, 'm', 'h', 2, ARRAY[1.0, 0.0]::real[])",
        )
        .bind(id)
        .execute(&pool)
        .await
        .expect("embedding");
        ids.push(id);
    }
    let repo = vyasa_db::repo::AiDataRepo::new(pool.clone());
    let got = |rows: Vec<vyasa_db::repo::EmbeddingRow>| {
        let mut v: Vec<i64> = rows.into_iter().map(|r| r.post_id).collect();
        v.sort_unstable();
        v
    };
    let public = got(repo
        .published_embeddings(2, Some(&["emb-open".to_owned()]))
        .await
        .unwrap());
    let mut want = vec![ids[0], ids[4]];
    want.sort_unstable();
    assert_eq!(public, want);
    let editor = got(repo.published_embeddings(2, None).await.unwrap());
    let mut want = vec![ids[0], ids[1], ids[3], ids[4]];
    want.sort_unstable();
    assert_eq!(editor, want);
}
