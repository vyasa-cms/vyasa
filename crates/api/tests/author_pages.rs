//! Public author pages: an account that has not confirmed its address has
//! no public presence (phase 98), so its author page is a 404 like a
//! username nobody holds.

#![allow(clippy::expect_used, clippy::unwrap_used)]

mod common;

use vyasa_db::models::Role;
use vyasa_db::repo::{NewUser, UsersRepo};
use vyasa_testkit::{TestDb, TestServer};

fn status(base: &str, path: &str) -> u16 {
    match ureq::get(&format!("{base}{path}")).call() {
        Ok(resp) | Err(ureq::Error::Status(_, resp)) => resp.status(),
        Err(ureq::Error::Transport(err)) => panic!("transport {err}"),
    }
}

fn user<'a>(id: i64, email: &'a str, username: &'a str) -> NewUser<'a> {
    NewUser {
        id,
        email,
        username,
        display_name: username,
        password_hash: None,
        role: Role::Author,
        bio: "",
    }
}

#[tokio::test]
async fn only_a_confirmed_account_has_an_author_page() {
    let db = TestDb::new().await;
    let users = UsersRepo::new(db.pool().clone());
    users
        .insert(&user(91_001, "writer@example.com", "writer"))
        .await
        .expect("confirmed author");
    users
        .insert_unconfirmed(&user(91_002, "pending@example.com", "pending-x7k2qa"), None)
        .await
        .expect("unconfirmed account");
    let server = TestServer::start(common::BIN, &db);
    let base = server.base();

    assert_eq!(status(base, "/author/writer"), 200);
    assert_eq!(status(base, "/author/WRITER"), 200);
    assert_eq!(status(base, "/author/pending-x7k2qa"), 404);
    assert_eq!(status(base, "/author/nobody"), 404);

    users
        .confirm_email_clearing_password(91_002)
        .await
        .expect("confirm");
    assert_eq!(status(base, "/author/pending-x7k2qa"), 200);
}
