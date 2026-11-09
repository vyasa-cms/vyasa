//! Menu items come back in the order the admin arranged them, with their description and badge.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use vyasa_db::repo::{MenusRepo, NewMenuItem};
use vyasa_testkit::TestDb;

#[tokio::test]
async fn top_level_items_follow_sort_order_not_creation_order() {
    let db = TestDb::new().await;
    let menus = MenusRepo::new(db.pool().clone());
    let menu = menus.create("Main", "main", None).await.expect("menu");

    // Created in one order, arranged in another. The site rendered the
    // creation order because top-level rows were grouped by their own id.
    for (label, sort_order) in [("Third", 2), ("First", 0), ("Second", 1)] {
        menus
            .add_item(
                menu.id,
                &NewMenuItem {
                    parent_id: None,
                    label: label.to_owned(),
                    url: format!("/{}", label.to_lowercase()),
                    sort_order,
                    description: String::new(),
                    badge: String::new(),
                },
            )
            .await
            .expect("item");
    }
    let labels: Vec<String> = menus
        .items(menu.id)
        .await
        .expect("items")
        .into_iter()
        .map(|i| i.label)
        .collect();
    assert_eq!(labels, ["First", "Second", "Third"]);
}

#[tokio::test]
async fn description_and_badge_are_stored_updated_and_default_to_empty() {
    let db = TestDb::new().await;
    let menus = MenusRepo::new(db.pool().clone());
    let menu = menus.create("Main", "main", None).await.expect("menu");

    let plain = menus
        .add_item(
            menu.id,
            &NewMenuItem {
                label: "Home".into(),
                url: "/".into(),
                ..Default::default()
            },
        )
        .await
        .expect("plain item");
    assert_eq!((plain.description.as_str(), plain.badge.as_str()), ("", ""));

    let rich = menus
        .add_item(
            menu.id,
            &NewMenuItem {
                label: "Engine".into(),
                url: "/engine".into(),
                sort_order: 1,
                description: "Batch and streaming".into(),
                badge: "0.2".into(),
                ..Default::default()
            },
        )
        .await
        .expect("rich item");
    assert_eq!(rich.badge, "0.2");

    // A partial update leaves what it does not name; an empty string clears.
    let updated = menus
        .update_item(
            rich.id,
            vyasa_db::repo::MenuItemPatch {
                description: Some(""),
                ..Default::default()
            },
        )
        .await
        .expect("update");
    assert_eq!(updated.description, "");
    assert_eq!(updated.badge, "0.2");
    assert_eq!(updated.label, "Engine");
}
