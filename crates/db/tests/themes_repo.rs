//! The active theme's identity is what the public site keys its built
//! bundle on, so it must follow activation exactly.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use serde_json::json;
use vyasa_db::repo::ThemesRepo;
use vyasa_testkit::TestDb;

#[tokio::test]
async fn the_active_identity_follows_activation() {
    let db = TestDb::new().await;
    let themes = ThemesRepo::new(db.pool().clone());
    assert_eq!(
        themes.active_identity().await.unwrap(),
        None,
        "nothing active yet"
    );

    let tokens = json!({"version": 1});
    let layout = json!({});
    for version in [1, 2] {
        themes
            .insert_version(
                "idtest",
                version,
                tokens.clone(),
                layout.clone(),
                None,
                None,
                None,
            )
            .await
            .expect("insert");
    }

    themes.set_active("idtest", 1).await.expect("activate v1");
    assert_eq!(
        themes.active_identity().await.unwrap(),
        Some(("idtest".to_owned(), 1))
    );

    // A new version activated is a different identity, so a bundle built
    // for the old one is not served for it.
    themes.set_active("idtest", 2).await.expect("activate v2");
    assert_eq!(
        themes.active_identity().await.unwrap(),
        Some(("idtest".to_owned(), 2))
    );
}

/// Bundled files belong to one version, copy forward when the studio
/// publishes, and vanish with the version.
#[tokio::test]
async fn bundled_files_follow_their_version() {
    let db = TestDb::new().await;
    let pool = db.pool().clone();
    let themes = ThemesRepo::new(pool.clone());
    let v1 = themes
        .insert_version(
            "files",
            1,
            json!({"version": 1}),
            json!({}),
            None,
            None,
            None,
        )
        .await
        .expect("v1");
    themes
        .put_files(
            v1.id,
            &[vyasa_db::repo::ThemeFileInput {
                path: "images/hero.jpg".into(),
                content_type: "image/jpeg".into(),
                sha256: "ab".repeat(32),
                bytes: vec![1, 2, 3],
            }],
        )
        .await
        .expect("put");
    let got = themes
        .file(v1.id, "images/hero.jpg")
        .await
        .unwrap()
        .expect("stored");
    assert_eq!(got.bytes, vec![1, 2, 3]);
    assert_eq!(got.content_type, "image/jpeg");
    assert!(themes
        .file(v1.id, "images/other.jpg")
        .await
        .unwrap()
        .is_none());

    let v2 = themes
        .insert_version(
            "files",
            2,
            json!({"version": 1}),
            json!({}),
            None,
            None,
            Some(v1.id),
        )
        .await
        .expect("v2");
    assert_eq!(themes.copy_files(v1.id, v2.id).await.unwrap(), 1);
    assert_eq!(
        themes.file_paths(v2.id).await.unwrap(),
        vec!["images/hero.jpg".to_owned()]
    );

    // Replacing v1's set does not touch v2's copy; deleting v1 takes only
    // v1's rows.
    themes.put_files(v1.id, &[]).await.expect("clear v1");
    assert!(themes.file_paths(v1.id).await.unwrap().is_empty());
    assert_eq!(themes.file_paths(v2.id).await.unwrap().len(), 1);
    themes.delete(v2.id).await.expect("delete v2");
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM theme_files")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(left, 0);
}
