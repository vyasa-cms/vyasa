//! The custom post type cap (64 names, plugins' and administrators'
//! together) as administrators meet it. A binary of its own: it fills the
//! process-global registry, which would starve every other test sharing
//! the process.

#![allow(clippy::pedantic, clippy::unwrap_used, clippy::expect_used)]

use vyasa_common::AppError;
use vyasa_core::content::{ContentTypesService, NewType};
use vyasa_db::content_models::PostType;
use vyasa_testkit::TestDb;

fn new_type(slug: &str) -> NewType {
    NewType {
        slug: slug.to_owned(),
        singular: "Thing".to_owned(),
        plural: "Things".to_owned(),
        description: String::new(),
        public: true,
        has_archive: true,
    }
}

#[tokio::test]
async fn the_cap_is_a_validation_error_and_deleting_a_type_frees_its_place_without_a_restart() {
    let db = TestDb::new().await;
    let types = ContentTypesService::new(db.pool().clone());
    let none: &[&str] = &[];

    // Plugins take all but one place.
    let mut n = 0;
    while PostType::custom_types().len() < 63 {
        PostType::register(&format!("cap-plugin-{n}")).expect("plugin type");
        n += 1;
    }
    types
        .create(new_type("cap-last"), none)
        .await
        .expect("the 64th fits");
    assert_eq!(PostType::custom_types().len(), 64);

    let err = types
        .create(new_type("cap-over"), none)
        .await
        .expect_err("the 65th does not");
    assert!(matches!(err, AppError::Validation { .. }), "{err}");
    assert!(err.to_string().contains("64"), "{err}");
    assert!(PostType::parse("cap-over").is_err());
    assert!(
        matches!(types.get("cap-over").await, Err(AppError::NotFound { .. })),
        "no row is left behind"
    );

    // Deleting one frees its place at once.
    types.delete("cap-last").await.expect("delete");
    types
        .create(new_type("cap-over"), none)
        .await
        .expect("now it fits");
    assert!(PostType::parse("cap-over").is_ok());
}
