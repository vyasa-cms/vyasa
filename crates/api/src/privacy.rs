//! Personal-data tools: everything the site holds about an email
//! address, as one document, and its erasure.

use serde::Serialize;
use vyasa_common::AppError;

use crate::state::AppState;

/// What the site holds about one address.
#[derive(Serialize, utoipa::ToSchema, Default)]
pub struct PersonalData {
    pub email: String,
    /// The account, without its password hash.
    pub account: Option<serde_json::Value>,
    pub comments: Vec<serde_json::Value>,
    pub submissions: Vec<serde_json::Value>,
    pub subscriptions: Vec<serde_json::Value>,
    /// Posts authored, by id and title, when there is an account.
    pub posts: Vec<serde_json::Value>,
    pub exported_at: String,
}

/// # Errors
/// Database errors.
pub async fn export(state: &AppState, email: &str) -> Result<PersonalData, AppError> {
    let email = email.trim().to_lowercase();
    let pool = &state.pool;
    let mut out = PersonalData {
        email: email.clone(),
        exported_at: chrono::Utc::now().to_rfc3339(),
        ..Default::default()
    };
    if let Ok(u) = vyasa_db::repo::UsersRepo::new(pool.clone())
        .get_by_email(&email)
        .await
    {
        out.account = Some(serde_json::json!({
            "id": u.id, "email": u.email, "username": u.username,
            "display_name": u.display_name, "role": u.role.as_str(),
            "custom_role": u.custom_role, "role_name": crate::rest::roles::role_name(&u),
            "bio": u.bio,
            "created_at": u.created_at, "last_login_at": u.last_login_at,
        }));
        let posts: Vec<(i64, String, String)> = sqlx::query_as(
            "SELECT id, title, status FROM posts WHERE author_id = $1 ORDER BY created_at",
        )
        .bind(u.id)
        .fetch_all(pool)
        .await
        .map_err(|e| AppError::db(format!("privacy posts: {e}")))?;
        out.posts = posts
            .into_iter()
            .map(|(id, title, status)| serde_json::json!({ "id": id, "title": title, "status": status }))
            .collect();
    }
    let comments: Vec<(
        i64,
        i64,
        String,
        String,
        String,
        chrono::DateTime<chrono::Utc>,
    )> = sqlx::query_as(
        "SELECT id, post_id, author_name, content, status, created_at FROM comments
         WHERE lower(author_email) = $1 ORDER BY created_at",
    )
    .bind(&email)
    .fetch_all(pool)
    .await
    .map_err(|e| AppError::db(format!("privacy comments: {e}")))?;
    out.comments = comments
        .into_iter()
        .map(|(id, post_id, name, content, status, at)| {
            serde_json::json!({ "id": id, "post_id": post_id, "author_name": name, "content": content, "status": status, "created_at": at })
        })
        .collect();
    let subs: Vec<(
        i64,
        String,
        String,
        String,
        serde_json::Value,
        chrono::DateTime<chrono::Utc>,
    )> = sqlx::query_as(
        "SELECT id, form, name, message, data, created_at FROM form_submissions
         WHERE lower(email) = $1 ORDER BY created_at",
    )
    .bind(&email)
    .fetch_all(pool)
    .await
    .map_err(|e| AppError::db(format!("privacy submissions: {e}")))?;
    out.submissions = subs
        .into_iter()
        .map(|(id, form, name, message, data, at)| {
            serde_json::json!({ "id": id, "form": form, "name": name, "message": message, "data": data, "created_at": at })
        })
        .collect();
    let news: Vec<(i64, String, chrono::DateTime<chrono::Utc>)> = sqlx::query_as(
        "SELECT id, status, created_at FROM newsletter_subscribers WHERE lower(email) = $1",
    )
    .bind(&email)
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    out.subscriptions = news
        .into_iter()
        .map(|(id, status, at)| serde_json::json!({ "id": id, "status": status, "created_at": at }))
        .collect();
    Ok(out)
}

/// What erasure did.
#[derive(Serialize, utoipa::ToSchema, Default)]
pub struct Erasure {
    pub comments_anonymised: u64,
    pub submissions_deleted: u64,
    pub subscriptions_deleted: u64,
    /// The account was deleted (its posts are reassigned by the user
    /// service's own rules).
    pub account_deleted: bool,
    pub note: String,
}

/// Anonymises comments (kept, so threads stay whole), deletes
/// submissions and subscriptions, and deletes the account when there is
/// one. Refuses to delete the last administrator.
///
/// # Errors
/// Database errors; the user service's own refusals.
pub async fn erase(state: &AppState, email: &str) -> Result<Erasure, AppError> {
    let email = email.trim().to_lowercase();
    let pool = &state.pool;
    let comments_anonymised = sqlx::query(
        "UPDATE comments SET author_name = 'Anonymous', author_email = '', author_user_id = NULL
         WHERE lower(author_email) = $1",
    )
    .bind(&email)
    .execute(pool)
    .await
    .map_err(|e| AppError::db(format!("privacy comments: {e}")))?
    .rows_affected();
    let submissions_deleted = sqlx::query("DELETE FROM form_submissions WHERE lower(email) = $1")
        .bind(&email)
        .execute(pool)
        .await
        .map_err(|e| AppError::db(format!("privacy submissions: {e}")))?
        .rows_affected();
    let subscriptions_deleted =
        sqlx::query("DELETE FROM newsletter_subscribers WHERE lower(email) = $1")
            .bind(&email)
            .execute(pool)
            .await
            .map_or(0, |r| r.rows_affected());
    let (account_deleted, note) = match vyasa_db::repo::UsersRepo::new(pool.clone())
        .get_by_email(&email)
        .await
    {
        Ok(u) => match state.users.delete(u.id).await {
            Ok(()) => (true, String::new()),
            Err(e) => (false, format!("account kept: {e}")),
        },
        Err(_) => (false, String::new()),
    };
    Ok(Erasure {
        comments_anonymised,
        submissions_deleted,
        subscriptions_deleted,
        account_deleted,
        note,
    })
}

/// The blocks of a starter privacy policy page, filled with the site's
/// name and what this software actually does.
#[must_use]
pub fn policy_blocks(site: &str, has_newsletter: bool, has_comments: bool) -> serde_json::Value {
    let p = |t: &str| serde_json::json!({ "kind": "paragraph", "attrs": { "text": t }, "children": [] });
    let h = |t: &str| serde_json::json!({ "kind": "heading", "attrs": { "level": 2, "text": t }, "children": [] });
    let mut blocks = vec![
        p(&format!("This page explains what {site} collects when you read it, comment, or write to us, and how to have it removed.")),
        h("What we collect"),
        p("Reading this site sets no cookies and loads no third-party scripts. Page views are counted on our server without any identifier that follows you between visits."),
    ];
    if has_comments {
        blocks.push(p("If you leave a comment, we keep the name and email address you give with it. The name is shown with the comment; the email is not published and is used only to tell you about replies."));
    }
    blocks.push(p("If you send a message through a form on this site, we keep what you wrote and the address you gave so we can reply."));
    if has_newsletter {
        blocks.push(p("If you subscribe to the newsletter, we keep your address until you unsubscribe. Every email carries an unsubscribe link, and nothing is sent until you confirm the address."));
    }
    blocks.extend([
        h("Who can see it"),
        p("The people who run this site. We do not sell or share personal data with anyone else, and we only pass it on when the law requires."),
        h("Your rights"),
        p("You can ask for a copy of everything we hold about your email address, or for it to be erased. Comments are kept but anonymised so conversations stay readable; messages and subscriptions are deleted outright. Write to us and we will do it promptly."),
        h("Changes"),
        p("If this policy changes, the new version is published here with the date it took effect."),
    ]);
    serde_json::json!(blocks)
}
