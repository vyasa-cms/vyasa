//! Menu items come back in the order the admin arranged them.

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
