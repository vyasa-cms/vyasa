//! Content types and custom fields (phase 99): the type and field
//! services, the live registry, value validation on every entry write,
//! the publish-time required check, and values in revisions.
//!
//! The post type registry is process-global and shared by every test in
//! this binary, so each test uses slugs of its own. The 64-name cap has a
//! binary of its own (`content_type_cap.rs`) for the same reason.

#![allow(
    clippy::pedantic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_lines
)]

use serde_json::{json, Value};
use vyasa_common::AppError;
use vyasa_core::block::{Block, BlockDocument, BlockKind};
use vyasa_core::content::{
    is_allowed_url, ContentFieldsService, ContentTypesService, FieldChanges, FieldKind, NewField,
    NewType, TypeChanges, RESERVED_TYPE_SLUGS,
};
use vyasa_core::post::{CreatePost, PostService, RevisionService, Snapshot, UpdatePost};
use vyasa_db::content_models::{MediaStorage, PostRow, PostStatus, PostType, TypeOwner};
use vyasa_db::models::Role;
use vyasa_db::repo::{
    ContentTypesRepo, MediaRepo, NewContentType, NewMedia, NewUser, PostsRepo, RevisionsRepo,
    UsersRepo,
};
use vyasa_testkit::TestDb;

const NO_PLUGINS: &[&str] = &[];

struct Ctx {
    _db: TestDb,
    pool: sqlx::PgPool,
    types: ContentTypesService,
    fields: ContentFieldsService,
    posts: PostService,
    author: i64,
}

async fn setup() -> Ctx {
    let db = TestDb::new().await;
    let pool = db.pool().clone();
    let author = vyasa_common::next_id_i64();
    UsersRepo::new(pool.clone())
        .insert(&NewUser {
            id: author,
            email: &format!("ct{author}@example.com"),
            username: &format!("ct{author}"),
            display_name: "CT",
            password_hash: Some("x"),
            role: Role::Admin,
            bio: "",
        })
        .await
        .expect("user");
    Ctx {
        types: ContentTypesService::new(pool.clone()),
        fields: ContentFieldsService::new(pool.clone()),
        posts: PostService::new(PostsRepo::new(pool.clone())),
        _db: db,
        pool,
        author,
    }
}

fn new_type(slug: &str) -> NewType {
    NewType {
        slug: slug.to_owned(),
        singular: "Product".to_owned(),
        plural: "Products".to_owned(),
        description: String::new(),
        public: true,
        has_archive: true,
    }
}

fn field(key: &str, kind: FieldKind, options: Value) -> NewField {
    NewField {
        key: key.to_owned(),
        label: key.to_owned(),
        help: String::new(),
        kind,
        required: false,
        options,
    }
}

fn doc() -> BlockDocument {
    BlockDocument::new(vec![Block {
        kind: BlockKind::Paragraph,
        plugin_kind: None,
        attrs: json!({ "text": "hello" }),
        children: Vec::new(),
    }])
}

fn entry(ctx: &Ctx, post_type: PostType, status: PostStatus) -> CreatePost {
    CreatePost {
        post_type,
        status,
        title: format!("Entry {}", vyasa_common::next_id_i64()),
        slug: None,
        content: doc(),
        excerpt: None,
        author_id: ctx.author,
        parent_id: None,
        scheduled_for: (status == PostStatus::Scheduled)
            .then(|| chrono::Utc::now() + chrono::Duration::days(1)),
        password: None,
        term_ids: None,
        layout: None,
    }
}

fn set_fields(values: Value) -> UpdatePost {
    UpdatePost {
        fields: Some(values),
        ..UpdatePost::default()
    }
}

fn set_status(status: PostStatus) -> UpdatePost {
    UpdatePost {
        status: Some(status),
        scheduled_for: (status == PostStatus::Scheduled)
            .then(|| chrono::Utc::now() + chrono::Duration::days(1)),
        ..UpdatePost::default()
    }
}

fn stored(row: &PostRow) -> Value {
    row.meta.get("fields").cloned().unwrap_or(json!({}))
}

fn is_validation(err: &AppError) -> bool {
    matches!(err, AppError::Validation { .. })
}

fn is_conflict(err: &AppError) -> bool {
    matches!(err, AppError::Conflict { .. })
}

async fn media(ctx: &Ctx) -> i64 {
    let id = vyasa_common::next_id_i64();
    MediaRepo::new(ctx.pool.clone())
        .insert(&NewMedia {
            id,
            owner_id: ctx.author,
            file_name: "photo.jpg",
            mime: "image/jpeg",
            byte_size: 10,
            storage: MediaStorage::Local,
            path: &format!("m/{id}.jpg"),
            width: None,
            height: None,
            blurhash: None,
            alt: None,
            caption: None,
            derivatives: json!({}),
            sha256: None,
        })
        .await
        .expect("media");
    id
}

// ---------------------------------------------------------------- types

#[tokio::test]
async fn a_type_is_created_relabelled_and_deleted_and_the_registry_follows_without_a_restart() {
    let ctx = setup().await;
    assert!(PostType::parse("ty-product").is_err());

    let made = ctx
        .types
        .create(
            NewType {
                description: "Things we sell".to_owned(),
                has_archive: false,
                ..new_type("ty-product")
            },
            NO_PLUGINS,
        )
        .await
        .expect("create");
    assert_eq!(made.slug, "ty-product");
    assert_eq!(made.description, "Things we sell");
    assert!(!made.has_archive);
    assert_eq!(
        PostType::parse("ty-product").expect("live at once"),
        PostType::Custom("ty-product")
    );
    assert!(PostType::custom_types().contains(&"ty-product"));
    assert_eq!(PostType::owner("ty-product"), Some(TypeOwner::Admin));
    assert_eq!(ctx.types.list().await.expect("list").len(), 1);

    let relabelled = ctx
        .types
        .update(
            "ty-product",
            TypeChanges {
                singular: Some("Item".to_owned()),
                public: Some(false),
                ..TypeChanges::default()
            },
        )
        .await
        .expect("relabel");
    assert_eq!(relabelled.singular, "Item");
    assert_eq!(relabelled.plural, "Products");
    assert!(!relabelled.public);
    assert!(is_validation(
        &ctx.types
            .update(
                "ty-product",
                TypeChanges {
                    plural: Some("  ".to_owned()),
                    ..TypeChanges::default()
                },
            )
            .await
            .unwrap_err()
    ));
    assert!(matches!(
        ctx.types
            .update("ty-missing", TypeChanges::default())
            .await
            .unwrap_err(),
        AppError::NotFound { .. }
    ));

    ctx.types.delete("ty-product").await.expect("delete");
    assert!(
        PostType::parse("ty-product").is_err(),
        "gone from requests without a restart"
    );
    assert!(!PostType::custom_types().contains(&"ty-product"));
    assert!(matches!(
        ctx.types.delete("ty-product").await.unwrap_err(),
        AppError::NotFound { .. }
    ));

    // And it can be made again, live again at once.
    ctx.types
        .create(new_type("ty-product"), NO_PLUGINS)
        .await
        .expect("again");
    assert!(PostType::parse("ty-product").is_ok());
}

#[tokio::test]
async fn malformed_built_in_and_reserved_slugs_are_refused_and_nothing_is_registered() {
    let ctx = setup().await;
    let long = "a".repeat(33);
    let mut refused: Vec<&str> = vec!["", "x", "-ab", "1ab", "a_b", "Ab", "a b", "ab-", &long];
    refused.extend_from_slice(RESERVED_TYPE_SLUGS);
    for slug in refused {
        let err = ctx
            .types
            .create(new_type(slug), NO_PLUGINS)
            .await
            .unwrap_err();
        assert!(is_validation(&err), "{slug:?}: {err}");
        assert!(PostType::owner(slug).is_none(), "{slug:?} registered");
    }
    assert!(ctx.types.list().await.expect("list").is_empty());

    // The spec's words and the router's own first segments are all in.
    for word in [
        "post",
        "page",
        "block",
        "admin",
        "api",
        "feed",
        "author",
        "tag",
        "category",
        "search",
        "sitemap",
        "media",
        "assets",
        "plugin",
        "login",
        "metrics",
        "unlock",
        "archive",
        "comment",
        "form",
        "newsletter",
        "preview",
        "plugin-assets",
        "theme-assets",
        "logo",
        "brand",
        "registry",
        "robots",
        "favicon",
    ] {
        assert!(RESERVED_TYPE_SLUGS.contains(&word), "{word} not reserved");
    }
}

#[tokio::test]
async fn a_slug_a_plugin_declares_is_refused() {
    let ctx = setup().await;
    let err = ctx
        .types
        .create(new_type("ty-plugged"), &["ty-plugged"])
        .await
        .unwrap_err();
    assert!(is_conflict(&err), "{err}");
    assert!(err.to_string().contains("plugin"), "{err}");
    assert!(PostType::owner("ty-plugged").is_none());

    // A plugin's type still live in this process (say, disabled since)
    // is refused too, even when the caller did not list it.
    PostType::register("ty-plugged-2").expect("plugin type");
    let err = ctx
        .types
        .create(new_type("ty-plugged-2"), NO_PLUGINS)
        .await
        .unwrap_err();
    assert!(is_conflict(&err), "{err}");
    assert_eq!(PostType::owner("ty-plugged-2"), Some(TypeOwner::Plugin));
    assert!(ctx.types.list().await.expect("list").is_empty());
}

#[tokio::test]
async fn review_focus_2_two_admins_creating_one_slug_at_once_one_wins_and_it_is_registered_once() {
    let ctx = setup().await;
    let a = ContentTypesService::new(ctx.pool.clone());
    let b = ContentTypesService::new(ctx.pool.clone());
    let (ra, rb) = tokio::join!(
        a.create(new_type("ty-race"), NO_PLUGINS),
        b.create(new_type("ty-race"), NO_PLUGINS)
    );
    assert_eq!(
        [ra.is_ok(), rb.is_ok()].iter().filter(|ok| **ok).count(),
        1,
        "{ra:?} / {rb:?}"
    );
    let loser = ra.err().or(rb.err()).expect("one lost");
    assert!(is_conflict(&loser), "{loser}");
    assert_eq!(
        PostType::custom_types()
            .iter()
            .filter(|t| **t == "ty-race")
            .count(),
        1
    );
    assert_eq!(PostType::owner("ty-race"), Some(TypeOwner::Admin));
    assert_eq!(ctx.types.list().await.expect("list").len(), 1);

    // The loser's failure did not take the winner's type out of the
    // registry.
    assert!(PostType::parse("ty-race").is_ok());
}

#[tokio::test]
async fn a_type_with_entries_in_any_status_is_not_deleted_and_the_count_is_given() {
    let ctx = setup().await;
    let made = ctx
        .types
        .create(new_type("ty-inuse"), NO_PLUGINS)
        .await
        .expect("type");
    let t = PostType::parse(&made.slug).expect("live");
    let draft = ctx
        .posts
        .create(entry(&ctx, t, PostStatus::Draft))
        .await
        .expect("draft");
    let other = ctx
        .posts
        .create(entry(&ctx, t, PostStatus::Draft))
        .await
        .expect("other");
    ctx.posts.trash(other.id).await.expect("trash");

    let err = ctx.types.delete("ty-inuse").await.unwrap_err();
    assert!(is_conflict(&err), "{err}");
    assert!(err.to_string().contains('2'), "the count: {err}");
    assert_eq!(ctx.types.entry_count("ty-inuse").await.expect("count"), 2);
    assert!(PostType::parse("ty-inuse").is_ok(), "still live");

    ctx.posts.delete(draft.id).await.expect("delete");
    ctx.posts.delete(other.id).await.expect("delete");
    ctx.types.delete("ty-inuse").await.expect("now it goes");
    assert!(PostType::parse("ty-inuse").is_err());
}

#[tokio::test]
async fn stored_types_are_loaded_into_the_registry_and_a_plugins_slug_is_reported() {
    let ctx = setup().await;
    let repo = ContentTypesRepo::new(ctx.pool.clone());
    for slug in ["ty-boot", "ty-boot-taken"] {
        repo.insert(&NewContentType {
            slug,
            singular: "B",
            plural: "Bs",
            description: "",
            public: true,
            has_archive: true,
        })
        .await
        .expect("row");
    }
    PostType::register("ty-boot-taken").expect("a plugin got there first");
    assert!(PostType::parse("ty-boot").is_err());

    let skipped = ctx.types.load_into_registry().await.expect("load");
    assert!(PostType::parse("ty-boot").is_ok());
    assert_eq!(PostType::owner("ty-boot"), Some(TypeOwner::Admin));
    assert_eq!(skipped.len(), 1, "{skipped:?}");
    assert_eq!(skipped[0].0, "ty-boot-taken");
    assert!(is_conflict(&skipped[0].1));
    assert_eq!(PostType::owner("ty-boot-taken"), Some(TypeOwner::Plugin));

    // Loading twice changes nothing.
    assert_eq!(
        ctx.types.load_into_registry().await.expect("again").len(),
        1
    );
}

#[tokio::test]
async fn type_labels_are_required_and_bounded() {
    let ctx = setup().await;
    for bad in [
        NewType {
            singular: " ".to_owned(),
            ..new_type("ty-labels")
        },
        NewType {
            plural: "x".repeat(81),
            ..new_type("ty-labels")
        },
        NewType {
            description: "x".repeat(501),
            ..new_type("ty-labels")
        },
        NewType {
            singular: "a\u{0}b".to_owned(),
            ..new_type("ty-labels")
        },
    ] {
        let err = ctx.types.create(bad, NO_PLUGINS).await.unwrap_err();
        assert!(is_validation(&err), "{err}");
    }
    assert!(PostType::owner("ty-labels").is_none());
}

// --------------------------------------------------------------- fields

#[tokio::test]
async fn fields_are_created_appended_reordered_updated_and_deleted() {
    let ctx = setup().await;
    let f = &ctx.fields;
    f.create("post", field("subtitle", FieldKind::Text, json!({})))
        .await
        .expect("text");
    f.create(
        "post",
        field("rating", FieldKind::Number, json!({"min": 1, "max": 5})),
    )
    .await
    .expect("number");
    f.create(
        "post",
        field(
            "size",
            FieldKind::Choice,
            json!({"choices": ["S", "M", "L"], "multiple": false}),
        ),
    )
    .await
    .expect("choice");
    // `page` has fields of its own.
    f.create("page", field("subtitle", FieldKind::Text, json!({})))
        .await
        .expect("page");

    let keys = |rows: Vec<vyasa_db::repo::ContentFieldRow>| -> Vec<String> {
        rows.into_iter().map(|r| r.key).collect()
    };
    assert_eq!(
        keys(f.list("post").await.expect("list")),
        ["subtitle", "rating", "size"]
    );
    assert_eq!(
        keys(
            f.reorder("post", &["size".into(), "subtitle".into(), "rating".into()])
                .await
                .expect("reorder")
        ),
        ["size", "subtitle", "rating"]
    );
    for wrong in [
        vec!["size".to_owned(), "subtitle".to_owned()],
        vec![
            "size".to_owned(),
            "subtitle".to_owned(),
            "rating".to_owned(),
            "extra".to_owned(),
        ],
        vec!["size".to_owned(), "size".to_owned(), "rating".to_owned()],
    ] {
        let err = f.reorder("post", &wrong).await.unwrap_err();
        assert!(is_validation(&err), "{wrong:?}: {err}");
    }

    let changed = f
        .update(
            "post",
            "rating",
            FieldChanges {
                label: Some("Stars".to_owned()),
                required: Some(true),
                options: Some(json!({"min": 0, "max": 10, "step": 0.5})),
                ..FieldChanges::default()
            },
        )
        .await
        .expect("update");
    assert_eq!(changed.label, "Stars");
    assert!(changed.required);
    assert_eq!(changed.kind, "number");

    f.delete("post", "rating").await.expect("delete");
    assert!(matches!(
        f.get("post", "rating").await.unwrap_err(),
        AppError::NotFound { .. }
    ));
    assert!(matches!(
        f.delete("post", "rating").await.unwrap_err(),
        AppError::NotFound { .. }
    ));
}

#[tokio::test]
async fn field_keys_types_and_options_follow_the_rules() {
    let ctx = setup().await;
    let f = &ctx.fields;
    let long = "a".repeat(41);
    for key in ["", "1a", "A", "a-b", "a b", "_a", long.as_str()] {
        let err = f
            .create("post", field(key, FieldKind::Text, json!({})))
            .await
            .unwrap_err();
        assert!(is_validation(&err), "{key:?}: {err}");
    }
    f.create("post", field("a", FieldKind::Text, json!({})))
        .await
        .expect("one letter is a key");
    f.create("post", field(&"b".repeat(40), FieldKind::Text, json!({})))
        .await
        .expect("forty characters is a key");
    let err = f
        .create("post", field("a", FieldKind::Url, json!({})))
        .await
        .unwrap_err();
    assert!(is_conflict(&err), "{err}");

    // Only real types take fields, and blocks are not entries.
    assert!(matches!(
        f.create("fd-nope", field("x", FieldKind::Text, json!({})))
            .await
            .unwrap_err(),
        AppError::NotFound { .. }
    ));
    assert!(is_validation(
        &f.create("block", field("x", FieldKind::Text, json!({})))
            .await
            .unwrap_err()
    ));
    // A plugin's type and an administrator's take them.
    PostType::register("fd-plugin").expect("plugin type");
    f.create("fd-plugin", field("x", FieldKind::Text, json!({})))
        .await
        .expect("plugin type");
    ctx.types
        .create(new_type("fd-admin"), NO_PLUGINS)
        .await
        .expect("type");
    f.create("fd-admin", field("x", FieldKind::Text, json!({})))
        .await
        .expect("admin type");

    for (kind, options) in [
        (FieldKind::Text, json!({"max_length": 0})),
        (FieldKind::Text, json!({"max_length": 1001})),
        (FieldKind::Text, json!({"max_length": "ten"})),
        (FieldKind::Textarea, json!({"max_length": 100_001})),
        (FieldKind::Text, json!({"maxlength": 10})),
        (FieldKind::Number, json!({"min": 5, "max": 1})),
        (FieldKind::Number, json!({"step": 0})),
        (FieldKind::Number, json!({"step": -1})),
        (FieldKind::Number, json!({"min": "1"})),
        (FieldKind::Boolean, json!({"default": true})),
        (FieldKind::Date, json!({"min": "2020-01-01"})),
        (FieldKind::Choice, json!({})),
        (FieldKind::Choice, json!({"choices": []})),
        (FieldKind::Choice, json!({"choices": ["a", "a"]})),
        (FieldKind::Choice, json!({"choices": ["a", ""]})),
        (FieldKind::Choice, json!({"choices": ["a", 1]})),
        (
            FieldKind::Choice,
            json!({"choices": ["a"], "multiple": "yes"}),
        ),
        (FieldKind::Url, json!({"schemes": ["ftp"]})),
        (FieldKind::Media, json!({"accept": "image"})),
        (FieldKind::Entry, json!({"entry_type": "fd-unknown"})),
        (FieldKind::Entry, json!({"entry_type": "block"})),
        (FieldKind::Text, json!([])),
    ] {
        let err = f
            .create("page", field("opt", kind, options.clone()))
            .await
            .unwrap_err();
        assert!(is_validation(&err), "{kind:?} {options}: {err}");
    }
    let mut label = field("lbl", FieldKind::Text, json!({}));
    label.label = "  ".to_owned();
    assert!(is_validation(&f.create("page", label).await.unwrap_err()));
    assert!(f.list("page").await.expect("list").is_empty());
}

#[tokio::test]
async fn a_kind_change_is_refused_while_an_entry_holds_a_value() {
    let ctx = setup().await;
    let f = &ctx.fields;
    f.create("post", field("colour", FieldKind::Text, json!({})))
        .await
        .expect("field");
    let post = ctx
        .posts
        .create_with_fields(
            entry(&ctx, PostType::Post, PostStatus::Draft),
            Some(json!({"colour": "red"})),
        )
        .await
        .expect("entry");
    ctx.posts.trash(post.id).await.expect("trash");

    let to_number = || FieldChanges {
        kind: Some(FieldKind::Number),
        ..FieldChanges::default()
    };
    let err = f.update("post", "colour", to_number()).await.unwrap_err();
    assert!(is_conflict(&err), "a trashed entry's value counts: {err}");
    assert!(err.to_string().contains('1'), "{err}");
    assert_eq!(f.get("post", "colour").await.expect("field").kind, "text");

    // Relabelling is always fine.
    f.update(
        "post",
        "colour",
        FieldChanges {
            label: Some("Colour".to_owned()),
            ..FieldChanges::default()
        },
    )
    .await
    .expect("label");

    // Once no entry holds a value, the kind may change.
    ctx.posts.restore(post.id).await.expect("restore");
    ctx.posts
        .update(post.id, set_fields(json!({})))
        .await
        .expect("clear");
    let changed = f.update("post", "colour", to_number()).await.expect("kind");
    assert_eq!(changed.kind, "number");
    assert_eq!(changed.options, json!({}), "options reset for the new kind");
    // A kind that needs options gets them checked for the new kind.
    let err = f
        .update(
            "post",
            "colour",
            FieldChanges {
                kind: Some(FieldKind::Choice),
                ..FieldChanges::default()
            },
        )
        .await
        .unwrap_err();
    assert!(is_validation(&err), "{err}");
}

#[tokio::test]
async fn review_focus_4_a_deleted_fields_key_is_not_reused_until_its_values_are_cleaned_up() {
    let ctx = setup().await;
    let f = &ctx.fields;
    f.create("post", field("colour", FieldKind::Text, json!({})))
        .await
        .expect("field");
    let post = ctx
        .posts
        .create_with_fields(
            entry(&ctx, PostType::Post, PostStatus::Draft),
            Some(json!({"colour": "red"})),
        )
        .await
        .expect("entry");

    // A defined field's values cannot be cleaned up from under it.
    assert!(is_conflict(
        &f.clean_up("post", "colour").await.unwrap_err()
    ));

    f.delete("post", "colour").await.expect("delete field");
    let kept = ctx.posts.get(post.id).await.expect("entry");
    assert_eq!(stored(&kept), json!({"colour": "red"}), "values are kept");
    assert_eq!(
        f.orphaned_values("post").await.expect("orphans"),
        [("colour".to_owned(), 1)]
    );

    // "red" would not validate as a number; it must not be adopted
    // silently, so the key cannot come back until the values are gone.
    let err = f
        .create("post", field("colour", FieldKind::Number, json!({})))
        .await
        .unwrap_err();
    assert!(is_conflict(&err), "{err}");
    assert!(err.to_string().contains("clean"), "{err}");
    // Not even with the same kind: the options may differ.
    assert!(is_conflict(
        &f.create("post", field("colour", FieldKind::Text, json!({})))
            .await
            .unwrap_err()
    ));

    assert_eq!(f.clean_up("post", "colour").await.expect("clean up"), 1);
    assert_eq!(
        stored(&ctx.posts.get(post.id).await.expect("entry")),
        json!({})
    );
    assert!(f.orphaned_values("post").await.expect("orphans").is_empty());
    f.create("post", field("colour", FieldKind::Number, json!({})))
        .await
        .expect("now the key is free");
}

// --------------------------------------------------------------- values

async fn every_kind(ctx: &Ctx, slug: &str) -> PostType {
    ctx.types
        .create(new_type(slug), NO_PLUGINS)
        .await
        .expect("type");
    let f = &ctx.fields;
    for (key, kind, options) in [
        ("title2", FieldKind::Text, json!({"max_length": 10})),
        ("body", FieldKind::Textarea, json!({"max_length": 50})),
        (
            "price",
            FieldKind::Number,
            json!({"min": 0, "max": 100, "step": 0.5}),
        ),
        ("count", FieldKind::Number, json!({})),
        ("in_stock", FieldKind::Boolean, json!({})),
        ("released", FieldKind::Date, json!({})),
        (
            "size",
            FieldKind::Choice,
            json!({"choices": ["S", "M", "L"]}),
        ),
        (
            "colours",
            FieldKind::Choice,
            json!({"choices": ["red", "green", "blue"], "multiple": true}),
        ),
        ("link", FieldKind::Url, json!({})),
        ("photo", FieldKind::Media, json!({})),
        ("related", FieldKind::Entry, json!({"entry_type": "page"})),
        ("any_entry", FieldKind::Entry, json!({})),
    ] {
        f.create(slug, field(key, kind, options)).await.expect(key);
    }
    PostType::parse(slug).expect("live")
}

#[tokio::test]
async fn every_kind_takes_valid_values_and_ids_are_stored_as_strings() {
    let ctx = setup().await;
    let t = every_kind(&ctx, "va-valid").await;
    let photo = media(&ctx).await;
    let page = ctx
        .posts
        .create(entry(&ctx, PostType::Page, PostStatus::Draft))
        .await
        .expect("page");

    let made = ctx
        .posts
        .create_with_fields(
            entry(&ctx, t, PostStatus::Draft),
            Some(json!({
                "title2": "Ten chars!",
                "body": "two\nlines\twith a tab",
                "price": 12.5,
                "count": -3,
                "in_stock": false,
                "released": "2024-02-29",
                "size": "M",
                "colours": ["blue", "red"],
                "link": "https://example.com/a?b=c",
                "photo": photo,
                "related": page.id.to_string(),
                "any_entry": page.id,
            })),
        )
        .await
        .expect("create");
    assert_eq!(
        stored(&made),
        json!({
            "title2": "Ten chars!",
            "body": "two\nlines\twith a tab",
            "price": 12.5,
            "count": -3,
            "in_stock": false,
            "released": "2024-02-29",
            "size": "M",
            "colours": ["blue", "red"],
            "link": "https://example.com/a?b=c",
            "photo": photo.to_string(),
            "related": page.id.to_string(),
            "any_entry": page.id.to_string(),
        }),
        "ids are kept as strings: a browser cannot hold a 64-bit id as a number"
    );

    // Site-relative links, and empty values meaning "no value" (dropped).
    let updated = ctx
        .posts
        .update(
            made.id,
            set_fields(json!({
                "link": "/about/team?x=1#top",
                "title2": "",
                "colours": [],
                "size": null,
            })),
        )
        .await
        .expect("update");
    assert_eq!(stored(&updated), json!({"link": "/about/team?x=1#top"}));
    assert!(updated.updated_at >= made.updated_at);
}

#[tokio::test]
async fn invalid_values_of_every_kind_are_refused_naming_the_field() {
    let ctx = setup().await;
    let t = every_kind(&ctx, "va-invalid").await;
    let trashed_media = media(&ctx).await;
    MediaRepo::new(ctx.pool.clone())
        .trash(trashed_media)
        .await
        .expect("trash media");
    let a_post = ctx
        .posts
        .create(entry(&ctx, PostType::Post, PostStatus::Draft))
        .await
        .expect("post");
    let a_trashed_page = ctx
        .posts
        .create(entry(&ctx, PostType::Page, PostStatus::Draft))
        .await
        .expect("page");
    ctx.posts.trash(a_trashed_page.id).await.expect("trash");
    let missing = vyasa_common::next_id_i64();

    let cases: Vec<(&str, Value)> = vec![
        ("title2", json!("eleven chars")),
        ("title2", json!("new\nline")),
        ("title2", json!(5)),
        ("body", json!("bell\u{7}")),
        ("body", json!("x".repeat(51))),
        ("price", json!(100.5)),
        ("price", json!(-1)),
        ("price", json!(0.3)),
        ("price", json!("12")),
        ("count", json!(true)),
        ("in_stock", json!("true")),
        ("in_stock", json!(1)),
        ("released", json!("2024-02-30")),
        ("released", json!("2023-02-29")),
        ("released", json!("2024-1-05")),
        ("released", json!("05/01/2024")),
        ("released", json!("2024-01-05T00:00:00Z")),
        ("released", json!("yesterday")),
        ("size", json!("XL")),
        ("size", json!(["S"])),
        ("size", json!("s")),
        ("colours", json!(["red", "pink"])),
        ("colours", json!(["red", "red"])),
        ("colours", json!("red")),
        ("link", json!("javascript:alert(1)")),
        ("link", json!("//evil.example")),
        ("link", json!("/\\evil.example")),
        ("link", json!("/\t/evil.example")),
        ("link", json!("/ok path")),
        ("link", json!("https://")),
        ("link", json!("https:///evil")),
        ("link", json!("ftp://example.com")),
        ("link", json!("data:text/html,x")),
        ("link", json!("example.com")),
        (
            "link",
            json!(format!("https://example.com/{}", "a".repeat(2048))),
        ),
        ("photo", json!(missing)),
        ("photo", json!(trashed_media)),
        ("photo", json!("12abc")),
        ("photo", json!(-4)),
        ("photo", json!(1.5)),
        ("related", json!(missing)),
        ("related", json!(a_post.id)),
        ("related", json!(a_trashed_page.id)),
        ("any_entry", json!("not an id")),
    ];
    for (key, value) in cases {
        let err = ctx
            .posts
            .create_with_fields(
                entry(&ctx, t, PostStatus::Draft),
                Some(json!({ key: value.clone() })),
            )
            .await
            .unwrap_err();
        assert!(is_validation(&err), "{key} = {value}: {err}");
        assert!(
            err.to_string().contains(&format!("fields.{key}")),
            "{key} = {value}: the message names the field: {err}"
        );
    }

    // Every problem is reported at once, not one per attempt.
    let err = ctx
        .posts
        .create_with_fields(
            entry(&ctx, t, PostStatus::Draft),
            Some(json!({"size": "XL", "price": "12"})),
        )
        .await
        .unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("fields.size") && msg.contains("fields.price"),
        "{msg}"
    );

    // Not an object at all.
    for not_object in [json!(["a"]), json!("a"), json!(1)] {
        let err = ctx
            .posts
            .create_with_fields(entry(&ctx, t, PostStatus::Draft), Some(not_object))
            .await
            .unwrap_err();
        assert!(is_validation(&err), "{err}");
    }

    // Nothing was stored by any of these.
    let made = ctx
        .posts
        .count(&vyasa_db::repo::PostFilter {
            post_type: Some(t),
            ..vyasa_db::repo::PostFilter::default()
        })
        .await
        .expect("count");
    assert_eq!(made, 0);
}

#[tokio::test]
async fn unknown_keys_are_refused() {
    let ctx = setup().await;
    let t = every_kind(&ctx, "va-unknown").await;
    let err = ctx
        .posts
        .create_with_fields(
            entry(&ctx, t, PostStatus::Draft),
            Some(json!({"price": 1, "nope": "x"})),
        )
        .await
        .unwrap_err();
    assert!(is_validation(&err), "{err}");
    assert!(err.to_string().contains("fields.nope"), "{err}");

    // A type without fields takes none.
    let err = ctx
        .posts
        .create_with_fields(
            entry(&ctx, PostType::Page, PostStatus::Draft),
            Some(json!({"anything": 1})),
        )
        .await
        .unwrap_err();
    assert!(is_validation(&err), "{err}");
    // But no values at all is always fine.
    ctx.posts
        .create_with_fields(
            entry(&ctx, PostType::Page, PostStatus::Draft),
            Some(json!({})),
        )
        .await
        .expect("empty object");

    // `meta.fields` is not a way to write values: editors echo the whole
    // meta back on every save, so a `fields` key there is ignored, valid
    // or not, and only `UpdatePost::fields` writes them.
    let post = ctx
        .posts
        .create(entry(&ctx, t, PostStatus::Draft))
        .await
        .expect("entry");
    let ignored = ctx
        .posts
        .update(
            post.id,
            UpdatePost {
                meta: Some(json!({"seo": "x", "fields": {"nope": 1}})),
                ..UpdatePost::default()
            },
        )
        .await
        .expect("an unknown key in meta.fields is ignored, not refused");
    assert_eq!(ignored.meta, json!({"seo": "x"}));
    let ignored = ctx
        .posts
        .update(
            post.id,
            UpdatePost {
                meta: Some(json!({"seo": "x", "fields": {"price": 2}})),
                ..UpdatePost::default()
            },
        )
        .await
        .expect("meta write");
    assert_eq!(
        ignored.meta,
        json!({"seo": "x"}),
        "a valid value in meta.fields is not written either"
    );
    let ok = ctx
        .posts
        .update(post.id, set_fields(json!({"price": 2})))
        .await
        .expect("values through fields");
    assert_eq!(ok.meta, json!({"seo": "x", "fields": {"price": 2}}));
    // A meta write without `fields` keeps the values.
    let kept = ctx
        .posts
        .update(
            post.id,
            UpdatePost {
                meta: Some(json!({"seo": "y"})),
                ..UpdatePost::default()
            },
        )
        .await
        .expect("meta only");
    assert_eq!(kept.meta, json!({"seo": "y", "fields": {"price": 2}}));
    // And meta itself must be an object.
    assert!(is_validation(
        &ctx.posts
            .update(
                post.id,
                UpdatePost {
                    meta: Some(json!(["x"])),
                    ..UpdatePost::default()
                },
            )
            .await
            .unwrap_err()
    ));
}

#[tokio::test]
async fn required_fields_block_publishing_and_scheduling_but_not_drafts() {
    let ctx = setup().await;
    ctx.types
        .create(new_type("va-required"), NO_PLUGINS)
        .await
        .expect("type");
    let t = PostType::parse("va-required").expect("live");
    let mut sku = field("sku", FieldKind::Text, json!({}));
    sku.required = true;
    ctx.fields.create("va-required", sku).await.expect("field");
    ctx.fields
        .create("va-required", field("note", FieldKind::Text, json!({})))
        .await
        .expect("optional field");

    // Drafts may lack it, including with other values.
    let draft = ctx
        .posts
        .create_with_fields(
            entry(&ctx, t, PostStatus::Draft),
            Some(json!({"note": "later"})),
        )
        .await
        .expect("draft without the required value");

    for status in [PostStatus::Published, PostStatus::Scheduled] {
        let err = ctx.posts.create(entry(&ctx, t, status)).await.unwrap_err();
        assert!(is_validation(&err), "{status:?}: {err}");
        assert!(err.to_string().contains("fields.sku"), "{err}");

        let err = ctx
            .posts
            .update(draft.id, set_status(status))
            .await
            .unwrap_err();
        assert!(is_validation(&err), "{status:?}: {err}");
        assert_eq!(
            ctx.posts.get(draft.id).await.expect("entry").status,
            PostStatus::Draft
        );
        // An empty value is no value.
        let err = ctx
            .posts
            .create_with_fields(entry(&ctx, t, status), Some(json!({"sku": ""})))
            .await
            .unwrap_err();
        assert!(is_validation(&err), "{err}");
    }

    // With the value, both work — in one write or in two.
    ctx.posts
        .create_with_fields(
            entry(&ctx, t, PostStatus::Scheduled),
            Some(json!({"sku": "A1"})),
        )
        .await
        .expect("scheduled with the value");
    let published = ctx
        .posts
        .update(
            draft.id,
            UpdatePost {
                status: Some(PostStatus::Published),
                fields: Some(json!({"sku": "A1", "note": "now"})),
                ..UpdatePost::default()
            },
        )
        .await
        .expect("publish with the value");
    assert_eq!(published.status, PostStatus::Published);

    // A published entry cannot lose it…
    let err = ctx
        .posts
        .update(draft.id, set_fields(json!({"note": "gone"})))
        .await
        .unwrap_err();
    assert!(is_validation(&err), "{err}");
    // …but going back to draft can.
    ctx.posts
        .update(
            draft.id,
            UpdatePost {
                status: Some(PostStatus::Draft),
                fields: Some(json!({"note": "draft again"})),
                ..UpdatePost::default()
            },
        )
        .await
        .expect("unpublish and clear");

    // A field made required after publishing does not block an edit that
    // neither publishes nor touches the values.
    let mut tagline = field("tagline", FieldKind::Text, json!({}));
    tagline.required = false;
    ctx.fields
        .create("va-required", tagline)
        .await
        .expect("field");
    let live = ctx
        .posts
        .create_with_fields(
            entry(&ctx, t, PostStatus::Published),
            Some(json!({"sku": "B2"})),
        )
        .await
        .expect("published");
    ctx.fields
        .update(
            "va-required",
            "tagline",
            FieldChanges {
                required: Some(true),
                ..FieldChanges::default()
            },
        )
        .await
        .expect("now required");
    ctx.posts
        .update(
            live.id,
            UpdatePost {
                title: Some("Typo fixed".to_owned()),
                ..UpdatePost::default()
            },
        )
        .await
        .expect("a title fix is not a publish");
}

#[tokio::test]
async fn a_stale_reference_does_not_block_saving_other_changes() {
    let ctx = setup().await;
    let t = every_kind(&ctx, "va-stale").await;
    let photo = media(&ctx).await;
    let page = ctx
        .posts
        .create(entry(&ctx, PostType::Page, PostStatus::Draft))
        .await
        .expect("page");
    let post = ctx
        .posts
        .create_with_fields(
            entry(&ctx, t, PostStatus::Draft),
            Some(json!({"photo": photo, "related": page.id})),
        )
        .await
        .expect("entry");
    MediaRepo::new(ctx.pool.clone())
        .delete(photo)
        .await
        .expect("media gone");
    ctx.posts.delete(page.id).await.expect("page gone");

    // The editor sends every value back with its other changes.
    let saved = ctx
        .posts
        .update(
            post.id,
            UpdatePost {
                title: Some("Renamed".to_owned()),
                fields: Some(stored(&post)),
                ..UpdatePost::default()
            },
        )
        .await
        .expect("unchanged stale references are kept");
    assert_eq!(stored(&saved), stored(&post));

    // Pointing a field at something missing is still refused.
    let err = ctx
        .posts
        .update(
            post.id,
            set_fields(json!({"photo": vyasa_common::next_id_i64()})),
        )
        .await
        .unwrap_err();
    assert!(is_validation(&err), "{err}");
}

// ------------------------------------------------------------ revisions

#[tokio::test]
async fn field_values_are_kept_in_revisions_and_autosaves_and_restored() {
    let ctx = setup().await;
    let t = every_kind(&ctx, "va-revs").await;
    let revisions = RevisionService::new(
        RevisionsRepo::new(ctx.pool.clone()),
        PostsRepo::new(ctx.pool.clone()),
    );
    let post = ctx
        .posts
        .create_with_fields(
            entry(&ctx, t, PostStatus::Draft),
            Some(json!({"price": 1, "size": "S"})),
        )
        .await
        .expect("entry");
    let first = revisions
        .snapshot_if_changed(post.id, ctx.author, &Snapshot::of(&post))
        .await
        .expect("snapshot")
        .expect("a revision");
    assert_eq!(first.fields, Some(json!({"price": 1, "size": "S"})));

    // Only the values changed: that is still a change worth recording.
    let second = ctx
        .posts
        .update(post.id, set_fields(json!({"price": 2, "size": "M"})))
        .await
        .expect("update");
    let rev2 = revisions
        .snapshot_if_changed(post.id, ctx.author, &Snapshot::of(&second))
        .await
        .expect("snapshot");
    assert!(rev2.is_some(), "a values-only change makes a revision");
    assert!(revisions
        .snapshot_if_changed(post.id, ctx.author, &Snapshot::of(&second))
        .await
        .expect("again")
        .is_none());

    // Autosaves carry them too.
    let auto_values = json!({"price": 3});
    let auto = revisions
        .autosave(
            post.id,
            ctx.author,
            &Snapshot {
                fields: Some(&auto_values),
                ..Snapshot::of(&second)
            },
        )
        .await
        .expect("autosave");
    assert_eq!(auto.fields, Some(json!({"price": 3})));

    // Restoring brings the first values back.
    let restored = revisions
        .restore(post.id, first.id, ctx.author)
        .await
        .expect("restore");
    assert_eq!(stored(&restored), json!({"price": 1, "size": "S"}));

    // A value that no longer fits its field (the choice was removed) is
    // not restored; the others are.
    ctx.fields
        .update(
            "va-revs",
            "size",
            FieldChanges {
                options: Some(json!({"choices": ["M", "L"]})),
                ..FieldChanges::default()
            },
        )
        .await
        .expect("options");
    let restored = revisions
        .restore(post.id, first.id, ctx.author)
        .await
        .expect("restore");
    assert_eq!(stored(&restored), json!({"price": 1}));

    // A revision from before fields existed keeps the current values.
    let old = RevisionsRepo::new(ctx.pool.clone())
        .insert(&vyasa_db::repo::NewRevision {
            id: vyasa_common::next_id_i64(),
            post_id: post.id,
            title: "Old",
            content: json!({"schema_version": 1, "blocks": []}),
            author_id: ctx.author,
            is_autosave: false,
            layout: None,
            fields: None,
        })
        .await
        .expect("old revision");
    let restored = revisions
        .restore(post.id, old.id, ctx.author)
        .await
        .expect("restore old");
    assert_eq!(restored.title, "Old");
    assert_eq!(stored(&restored), json!({"price": 1}));
}

#[tokio::test]
async fn an_echoed_meta_never_writes_field_values() {
    let ctx = setup().await;
    ctx.types
        .create(new_type("va-echo"), NO_PLUGINS)
        .await
        .expect("type");
    let t = PostType::parse("va-echo").expect("live");
    for (key, kind, options) in [
        ("colour", FieldKind::Text, json!({})),
        ("size", FieldKind::Choice, json!({"choices": ["S", "M"]})),
    ] {
        ctx.fields
            .create("va-echo", field(key, kind, options))
            .await
            .expect(key);
    }
    let post = ctx
        .posts
        .create_with_fields(
            entry(&ctx, t, PostStatus::Draft),
            Some(json!({"colour": "red", "size": "S"})),
        )
        .await
        .expect("entry");
    let echo = || {
        let mut meta = post.meta.clone();
        meta["seo"] = json!("edited");
        meta
    };

    // A field deleted and another's options tightened: the echo now holds
    // values that would not validate, and saving must still work.
    ctx.fields
        .delete("va-echo", "colour")
        .await
        .expect("delete");
    ctx.fields
        .update(
            "va-echo",
            "size",
            FieldChanges {
                options: Some(json!({"choices": ["M", "L"]})),
                ..FieldChanges::default()
            },
        )
        .await
        .expect("tighten");
    let saved = ctx
        .posts
        .update(
            post.id,
            UpdatePost {
                title: Some("Edited".to_owned()),
                meta: Some(echo()),
                ..UpdatePost::default()
            },
        )
        .await
        .expect("an echo of stale values does not block a save");
    assert_eq!(saved.meta["seo"], json!("edited"));
    assert_eq!(stored(&saved), json!({"colour": "red", "size": "S"}));

    // Newer values are not overwritten by a stale echo.
    ctx.posts
        .update(post.id, set_fields(json!({"size": "M"})))
        .await
        .expect("newer values");
    let saved = ctx
        .posts
        .update(
            post.id,
            UpdatePost {
                meta: Some(echo()),
                ..UpdatePost::default()
            },
        )
        .await
        .expect("stale echo");
    // (`colour`'s field was deleted above: its value stays stored until
    // a clean-up, whatever the saves send.)
    assert_eq!(stored(&saved), json!({"colour": "red", "size": "M"}));

    // A field made required later does not block a title edit (with the
    // echo the editor sends) on a published entry.
    let live = ctx
        .posts
        .create_with_fields(
            entry(&ctx, t, PostStatus::Published),
            Some(json!({"size": "L"})),
        )
        .await
        .expect("published");
    let mut sku = field("sku", FieldKind::Text, json!({}));
    sku.required = true;
    ctx.fields
        .create("va-echo", sku)
        .await
        .expect("required now");
    ctx.posts
        .update(
            live.id,
            UpdatePost {
                title: Some("Typo fixed".to_owned()),
                meta: Some(live.meta.clone()),
                ..UpdatePost::default()
            },
        )
        .await
        .expect("a title edit is not a publish");
}

#[tokio::test]
async fn restoring_re_checks_references_and_reports_what_it_dropped() {
    let ctx = setup().await;
    ctx.types
        .create(new_type("va-restore"), NO_PLUGINS)
        .await
        .expect("type");
    let t = PostType::parse("va-restore").expect("live");
    ctx.fields
        .create("va-restore", field("thing", FieldKind::Media, json!({})))
        .await
        .expect("media field");
    ctx.fields
        .create("va-restore", field("note", FieldKind::Text, json!({})))
        .await
        .expect("text field");
    let revisions = RevisionService::new(
        RevisionsRepo::new(ctx.pool.clone()),
        PostsRepo::new(ctx.pool.clone()),
    );

    // A media item whose id is also an ordinary post's id: once the field
    // takes entries, the old media id names that post.
    let arbitrary = ctx
        .posts
        .create(entry(&ctx, PostType::Post, PostStatus::Draft))
        .await
        .expect("an unrelated post");
    MediaRepo::new(ctx.pool.clone())
        .insert(&NewMedia {
            id: arbitrary.id,
            owner_id: ctx.author,
            file_name: "a.jpg",
            mime: "image/jpeg",
            byte_size: 1,
            storage: MediaStorage::Local,
            path: "m/a.jpg",
            width: None,
            height: None,
            blurhash: None,
            alt: None,
            caption: None,
            derivatives: json!({}),
            sha256: None,
        })
        .await
        .expect("media");
    let post = ctx
        .posts
        .create_with_fields(
            entry(&ctx, t, PostStatus::Draft),
            Some(json!({"thing": arbitrary.id, "note": "old"})),
        )
        .await
        .expect("entry");
    let old = revisions
        .snapshot_if_changed(post.id, ctx.author, &Snapshot::of(&post))
        .await
        .expect("snapshot")
        .expect("revision");

    // Values cleared so the kind may change; media becomes entry (pages).
    ctx.posts
        .update(post.id, set_fields(json!({"note": "new"})))
        .await
        .expect("clear");
    ctx.fields
        .update(
            "va-restore",
            "thing",
            FieldChanges {
                kind: Some(FieldKind::Entry),
                options: Some(json!({"entry_type": "page"})),
                ..FieldChanges::default()
            },
        )
        .await
        .expect("kind change");

    let restored = revisions
        .restore_reporting(post.id, old.id, ctx.author)
        .await
        .expect("restore");
    assert_eq!(
        stored(&restored.post),
        json!({"note": "old"}),
        "the old media id must not become a reference to an arbitrary post"
    );
    assert_eq!(restored.dropped_fields, ["thing"]);

    // Without a type restriction the post would be a valid entry; a
    // trashed one is still refused.
    ctx.fields
        .update(
            "va-restore",
            "thing",
            FieldChanges {
                options: Some(json!({})),
                ..FieldChanges::default()
            },
        )
        .await
        .expect("any type");
    ctx.posts.trash(arbitrary.id).await.expect("trash it");
    let restored = revisions
        .restore_reporting(post.id, old.id, ctx.author)
        .await
        .expect("restore");
    assert_eq!(stored(&restored.post), json!({"note": "old"}));
    assert_eq!(restored.dropped_fields, ["thing"]);

    // A value equal to the entry's current one is kept as it is.
    let current = ctx
        .posts
        .update(post.id, set_fields(json!({"note": "same"})))
        .await
        .expect("set");
    let same = revisions
        .snapshot_if_changed(post.id, ctx.author, &Snapshot::of(&current))
        .await
        .expect("snapshot")
        .expect("revision");
    let restored = revisions
        .restore_reporting(post.id, same.id, ctx.author)
        .await
        .expect("restore");
    assert_eq!(stored(&restored.post), json!({"note": "same"}));
    assert!(restored.dropped_fields.is_empty());
}

#[tokio::test]
async fn a_failed_create_leaves_nothing_in_the_registry() {
    let ctx = setup().await;
    // Another writer holds the slug in a transaction that has not
    // committed: this create cannot see it, and its insert fails.
    let mut held = ctx.pool.begin().await.expect("tx");
    sqlx::query("INSERT INTO content_types (slug, singular, plural) VALUES ('ty-held', 'H', 'Hs')")
        .execute(&mut *held)
        .await
        .expect("held row");
    let types = ctx.types.clone();
    let create = tokio::spawn(async move { types.create(new_type("ty-held"), NO_PLUGINS).await });
    tokio::task::spawn_blocking(|| std::thread::sleep(std::time::Duration::from_millis(300)))
        .await
        .expect("wait");
    held.commit().await.expect("commit");
    let err = create.await.expect("join").unwrap_err();
    assert!(is_conflict(&err), "{err}");
    assert_eq!(PostType::owner("ty-held"), None, "not live");
    assert!(!PostType::is_interned("ty-held"), "no name spent on it");

    // Refusals before the write spend nothing either.
    for (slug, plugins) in [("archive", NO_PLUGINS), ("ty-refused", &["ty-refused"][..])] {
        assert!(ctx.types.create(new_type(slug), plugins).await.is_err());
        assert!(!PostType::is_interned(slug), "{slug}");
    }
}

#[tokio::test]
async fn a_slug_with_stored_entries_or_a_live_taxonomy_is_refused() {
    let ctx = setup().await;
    // Entries of the slug are already stored (a plugin removed since, or
    // an import): the type would silently adopt them.
    for status in [PostStatus::Draft, PostStatus::Trash] {
        PostsRepo::new(ctx.pool.clone())
            .insert(&vyasa_db::repo::NewPost {
                id: vyasa_common::next_id_i64(),
                post_type: PostType::from_db("ty-orphaned").expect("decodes"),
                status,
                slug: format!("o-{}", vyasa_common::next_id_i64()),
                title: "Orphan".into(),
                content: json!({"schema_version": 1, "blocks": []}),
                excerpt: None,
                author_id: ctx.author,
                parent_id: None,
                meta: json!({}),
                published_at: None,
                scheduled_for: None,
                password_hash: None,
                layout: None,
            })
            .await
            .expect("orphan entry");
    }
    let err = ctx
        .types
        .create(new_type("ty-orphaned"), NO_PLUGINS)
        .await
        .unwrap_err();
    assert!(is_conflict(&err), "{err}");
    assert!(err.to_string().contains('2'), "the count: {err}");
    assert_eq!(PostType::owner("ty-orphaned"), None);

    vyasa_db::content_models::Taxonomy::register("ty-taxonomy").expect("taxonomy");
    let err = ctx
        .types
        .create(new_type("ty-taxonomy"), NO_PLUGINS)
        .await
        .unwrap_err();
    assert!(is_conflict(&err), "{err}");
    assert!(err.to_string().contains("taxonomy"), "{err}");
    assert!(ctx.types.list().await.expect("list").is_empty());
}

#[tokio::test]
async fn churning_types_past_the_old_ceiling_still_allows_a_new_one() {
    let ctx = setup().await;
    for n in 0..300 {
        let slug = format!("ty-churn-{n}");
        ctx.types
            .create(new_type(&slug), NO_PLUGINS)
            .await
            .expect("create");
        ctx.types.delete(&slug).await.expect("delete");
    }
    ctx.types
        .create(new_type("ty-churn-new"), NO_PLUGINS)
        .await
        .expect("a new type after the churn");
    assert!(PostType::parse("ty-churn-new").is_ok());
}

#[tokio::test]
async fn a_field_create_racing_its_types_delete_leaves_no_orphan_row() {
    let ctx = setup().await;
    for n in 0..20 {
        let slug = format!("ty-race-field-{n}");
        ctx.types
            .create(new_type(&slug), NO_PLUGINS)
            .await
            .expect("type");
        let (deleted, created) = tokio::join!(
            ctx.types.delete(&slug),
            ctx.fields
                .create(&slug, field("x", FieldKind::Text, json!({})))
        );
        deleted.expect("the type goes either way");
        if let Err(e) = created {
            assert!(matches!(e, AppError::NotFound { .. }), "{e}");
        }
        let left: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM content_fields WHERE type_slug = $1")
                .bind(&slug)
                .fetch_one(&ctx.pool)
                .await
                .expect("count");
        assert_eq!(left, 0, "no field row outlives its type");
    }
}

#[tokio::test]
async fn loading_skips_a_stored_type_whose_slug_is_now_reserved() {
    let ctx = setup().await;
    ContentTypesRepo::new(ctx.pool.clone())
        .insert(&NewContentType {
            slug: "newsletter",
            singular: "N",
            plural: "Ns",
            description: "",
            public: true,
            has_archive: true,
        })
        .await
        .expect("a row from before the word was reserved");
    let skipped = ctx.types.load_into_registry().await.expect("load");
    let (slug, reason) = skipped
        .iter()
        .find(|(slug, _)| slug == "newsletter")
        .expect("reported");
    assert_eq!(slug, "newsletter");
    assert!(is_validation(reason), "{reason}");
    assert!(PostType::parse("newsletter").is_err());
}

// ----------------------------------------------------------------- urls

#[test]
fn the_url_rule_takes_http_and_site_relative_links_only() {
    for ok in [
        "http://example.com",
        "https://example.com/a?b=c#d",
        "HTTPS://Example.com",
        "/",
        "/about",
        "/a/b?c=d#e",
    ] {
        assert!(is_allowed_url(ok), "{ok:?}");
    }
    for bad in [
        "",
        "about",
        "example.com",
        "//evil.example",
        "///evil.example",
        "/\\evil.example",
        "/\\/evil.example",
        "\\\\evil.example",
        "/\t/evil.example",
        "/\n/evil",
        "/\r//evil",
        "/ /evil",
        "/ok\u{0}",
        "/\u{7f}/evil",
        "javascript:alert(1)",
        "JAVASCRIPT:alert(1)",
        "data:text/html,x",
        "ftp://example.com",
        "mailto:a@example.com",
        "http://",
        "https:",
        "https:/example.com",
        "https:///evil",
        "https://\\evil",
        "https://exa mple.com",
        " https://example.com",
        "vbscript:msgbox(1)",
        "JaVaScRiPt:alert(1)",
        "jav\tascript:alert(1)",
        // Look-alikes: fullwidth letters, colon and solidus, and
        // characters that reorder or hide text.
        "\u{ff48}\u{ff54}\u{ff54}\u{ff50}\u{ff53}://example.com",
        "https\u{ff1a}//example.com",
        "\u{ff0f}\u{ff0f}evil.example",
        "/\u{ff0f}evil.example",
        "https://example.com/\u{202e}gpj.exe",
        "/\u{200b}/evil.example",
        "https://exa\u{feff}mple.com",
    ] {
        assert!(!is_allowed_url(bad), "{bad:?}");
    }
}

#[test]
fn the_url_rule_refuses_every_format_character_and_slash_look_alike() {
    // Every Unicode general-category Cf character, sampled across its
    // ranges, plus the invisible fillers and variation selectors that are
    // not Cf, and the slash and backslash look-alikes.
    let refused = [
        '\u{ad}',
        '\u{600}',
        '\u{605}',
        '\u{61c}',
        '\u{6dd}',
        '\u{70f}',
        '\u{890}',
        '\u{891}',
        '\u{8e2}',
        '\u{180e}',
        '\u{200b}',
        '\u{200f}',
        '\u{202a}',
        '\u{2060}',
        '\u{2064}',
        '\u{2066}',
        '\u{206f}',
        '\u{feff}',
        '\u{fff9}',
        '\u{fffb}',
        '\u{110bd}',
        '\u{110cd}',
        '\u{13430}',
        '\u{1343f}',
        '\u{1bca0}',
        '\u{1bca3}',
        '\u{1d173}',
        '\u{1d17a}',
        '\u{e0001}',
        '\u{e0020}',
        '\u{e007f}',
        '\u{e0000}', // not Cf, but tag-block
        '\u{34f}',
        '\u{115f}',
        '\u{1160}',
        '\u{3164}',
        '\u{ffa0}',
        '\u{180b}',
        '\u{fe00}',
        '\u{fe0f}',
        '\u{e0100}',
        '\u{e01ef}',
        '\u{2800}', // invisible, not Cf
        '\u{2215}',
        '\u{2044}',
        '\u{ff0f}',
        '\u{2216}',
        '\u{29f5}',
        '\u{fe68}',
        '\u{ff3c}',
        '\u{29f8}',
        '\u{29f9}',
        '\u{2571}',
        '\u{2572}',
        '\u{27cb}',
        '\u{27cd}',
        '\u{2041}',
        '\u{1735}',
        '\u{2f03}',
        '\u{31d3}',
        '\u{31d4}', // slash and backslash look-alikes
    ];
    for c in refused {
        for url in [format!("/a{c}b"), format!("https://example.com/{c}x")] {
            assert!(!is_allowed_url(&url), "U+{:04X} in {url:?}", u32::from(c));
        }
    }
    // Ordinary non-ASCII text in a path is still a link.
    assert!(is_allowed_url("/caf\u{e9}/\u{65e5}\u{672c}"));
}

/// Deleting a field keeps its stored values until a clean-up: a save that
/// sends the defined fields must not wipe them.
#[tokio::test]
async fn saving_values_keeps_a_deleted_fields_values_until_the_clean_up() {
    let ctx = setup().await;
    ctx.types
        .create(new_type("ty-orphans"), NO_PLUGINS)
        .await
        .expect("type");
    for key in ["kept", "gone"] {
        ctx.fields
            .create("ty-orphans", field(key, FieldKind::Text, json!({})))
            .await
            .expect("field");
    }
    let t = PostType::parse("ty-orphans").expect("live");
    let row = ctx
        .posts
        .create_with_fields(
            entry(&ctx, t, PostStatus::Draft),
            Some(json!({"kept": "a", "gone": "b"})),
        )
        .await
        .expect("entry");
    ctx.fields
        .delete("ty-orphans", "gone")
        .await
        .expect("delete");

    let row = ctx
        .posts
        .update(row.id, set_fields(json!({"kept": "c"})))
        .await
        .expect("save");
    assert_eq!(stored(&row), json!({"kept": "c", "gone": "b"}));
    // `{}` clears the defined values, not the orphan.
    let row = ctx
        .posts
        .update(row.id, set_fields(json!({})))
        .await
        .expect("clear");
    assert_eq!(stored(&row), json!({"gone": "b"}));
    assert_eq!(
        ctx.fields
            .orphaned_values("ty-orphans")
            .await
            .expect("orphans"),
        vec![("gone".to_owned(), 1)]
    );
    assert_eq!(
        ctx.fields
            .clean_up("ty-orphans", "gone")
            .await
            .expect("clean"),
        1
    );
    let row = ctx.posts.get(row.id).await.expect("row");
    assert_eq!(stored(&row), json!({}));
}

/// Field definitions left under a slug by an earlier owner (a plugin's
/// type, the plugin removed since) would be adopted silently by a new
/// administrator's type of that slug. The create is refused with the
/// count and the keys; deleting those definitions (the field delete
/// works for a type that is no longer live) lets it through.
#[tokio::test]
async fn a_slug_with_left_over_field_definitions_is_refused_until_they_are_removed() {
    let ctx = setup().await;
    for key in ["colour", "size"] {
        sqlx::query(
            "INSERT INTO content_fields (type_slug, key, label, kind)
             VALUES ('ty-stale-defs', $1, 'Old', 'text')",
        )
        .bind(key)
        .execute(&ctx.pool)
        .await
        .expect("left-over definition");
    }
    let err = ctx
        .types
        .create(new_type("ty-stale-defs"), NO_PLUGINS)
        .await
        .unwrap_err();
    assert!(is_conflict(&err), "{err}");
    let message = err.to_string();
    assert!(message.contains("2 field definitions"), "{message}");
    assert!(message.contains("\"colour\""), "{message}");
    assert!(message.contains("\"size\""), "{message}");
    assert_eq!(PostType::owner("ty-stale-defs"), None);
    assert!(ctx.types.list().await.expect("list").is_empty());

    ctx.fields
        .delete("ty-stale-defs", "colour")
        .await
        .expect("delete");
    let err = ctx
        .types
        .create(new_type("ty-stale-defs"), NO_PLUGINS)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("1 field definition "), "{err}");
    ctx.fields
        .delete("ty-stale-defs", "size")
        .await
        .expect("delete");
    ctx.types
        .create(new_type("ty-stale-defs"), NO_PLUGINS)
        .await
        .expect("created once the definitions are gone");
    assert!(ctx
        .fields
        .list("ty-stale-defs")
        .await
        .expect("fields")
        .is_empty());
}
