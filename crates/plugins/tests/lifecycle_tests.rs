//! Install, upgrade and rollback over a real database.
//!
//! These are the operations that decide which bytes a site runs, and they
//! had no tests at all: a first install, an upgrade of something already
//! there, and going back when the new version turns out to be wrong.

#![allow(clippy::pedantic, clippy::unwrap_used, clippy::expect_used)]

use vyasa_db::repo::PluginsRepo;
use vyasa_plugins::lifecycle::{install, rollback};
use vyasa_testkit::TestDb;

async fn repo() -> (TestDb, PluginsRepo) {
    let db = TestDb::new().await;
    let repo = PluginsRepo::new(db.pool().clone());
    (db, repo)
}

fn caps(list: &[&str]) -> serde_json::Value {
    serde_json::json!(list)
}

#[tokio::test]
async fn installing_twice_upgrades_rather_than_conflicting() {
    let (_db, repo) = repo().await;

    let first = install(
        &repo,
        "demo",
        "1.0.0",
        b"\0asm-one",
        "aaa",
        &caps(&["log:write"]),
    )
    .await
    .expect("first install");
    assert_eq!(first.version, "1.0.0");

    // The same name again is an upgrade, not a conflict: `create` returns
    // one and the lifecycle turns it into a new version row.
    let second = install(
        &repo,
        "demo",
        "1.1.0",
        b"\0asm-two",
        "bbb",
        &caps(&["log:write", "kv:read"]),
    )
    .await
    .expect("upgrade");
    assert_eq!(second.id, first.id, "the same plugin row");

    let versions = repo.versions(first.id).await.expect("versions");
    assert_eq!(versions.len(), 2, "both versions kept: {versions:?}");

    // The capability list follows the newly installed version. It once did
    // not, so a plugin that dropped a capability kept it.
    let row = repo
        .list()
        .await
        .unwrap()
        .into_iter()
        .find(|p| p.id == first.id)
        .unwrap();
    assert_eq!(row.capabilities, caps(&["log:write", "kv:read"]));
}

#[tokio::test]
async fn the_same_version_twice_is_refused() {
    let (_db, repo) = repo().await;
    install(&repo, "demo", "1.0.0", b"\0asm", "aaa", &caps(&[]))
        .await
        .expect("first");
    // Re-installing an identical version would leave two rows claiming to
    // be the same build.
    assert!(
        install(
            &repo,
            "demo",
            "1.0.0",
            b"\0asm-different",
            "ccc",
            &caps(&[])
        )
        .await
        .is_err(),
        "a duplicate version must be refused"
    );
}

#[tokio::test]
async fn rollback_returns_to_the_previous_version_and_stops_there() {
    let (_db, repo) = repo().await;

    let row = install(&repo, "demo", "1.0.0", b"\0asm-one", "aaa", &caps(&[]))
        .await
        .expect("install");
    // Only one version: there is nowhere to roll back to, and saying so is
    // better than quietly reinstalling what is already running.
    assert!(rollback(&repo, row.id).await.is_err());

    install(&repo, "demo", "1.1.0", b"\0asm-two", "bbb", &caps(&[]))
        .await
        .expect("upgrade");
    let back = rollback(&repo, row.id).await.expect("rolls back");
    assert_eq!(back, "1.0.0");

    let current = repo
        .list()
        .await
        .unwrap()
        .into_iter()
        .find(|p| p.id == row.id)
        .unwrap();
    assert_eq!(current.version, "1.0.0", "the active version moved");

    // Both versions are still stored, so rolling forward is possible.
    assert_eq!(repo.versions(row.id).await.unwrap().len(), 2);
}

#[tokio::test]
async fn rollback_of_an_unknown_plugin_is_not_found() {
    let (_db, repo) = repo().await;
    let err = rollback(&repo, 404).await.unwrap_err().to_string();
    assert!(err.contains("plugin"), "{err}");
}
