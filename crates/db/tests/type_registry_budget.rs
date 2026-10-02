//! The ceiling on post type names administrators introduce: it bounds the
//! names leaked for administrator-created types, and nothing else. A
//! binary of its own because it spends the process's whole budget.

#![allow(clippy::pedantic, clippy::unwrap_used, clippy::expect_used)]

use vyasa_common::AppError;
use vyasa_db::content_models::{PostType, MAX_ADMIN_TYPE_NAMES};

#[test]
fn only_new_administrator_names_are_bounded() {
    for n in 0..MAX_ADMIN_TYPE_NAMES {
        let name = format!("bud-{n}");
        PostType::register_admin(&name).expect("within the budget");
        assert!(PostType::unregister_admin(&name));
    }
    let err = PostType::check_admin("bud-one-more").unwrap_err();
    assert!(matches!(err, AppError::Validation { .. }), "{err}");
    assert!(err.to_string().contains("restart"), "{err}");
    assert!(PostType::register_admin("bud-one-more").is_err());
    assert!(
        !PostType::is_interned("bud-one-more"),
        "a refusal leaks nothing"
    );

    // A name already seen costs nothing: re-creating a deleted type works.
    PostType::register_admin("bud-0").expect("a known name");
    assert!(PostType::unregister_admin("bud-0"));

    // Plugins and stored rows are not held to it.
    PostType::register("bud-plugin").expect("a plugin still registers");
    PostType::from_db("bud-row").expect("a row still decodes");
}
