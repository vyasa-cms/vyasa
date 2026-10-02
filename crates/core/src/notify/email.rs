//! Email rendering and queueing.
//!
//! Rendering and sending are deliberately separate. This module turns a
//! template name plus a context into a subject and body and puts it on the
//! job queue; the `jobs` crate owns the SMTP transport. Keeping the
//! transport out of `core` means the domain layer stays free of network IO,
//! and a send that fails inherits the queue's retry and dead-letter
//! behaviour for free.
//!
//! Templates live beside this file as `.tera` files and are embedded at
//! compile time, so a missing template is a build error rather than a
//! runtime surprise. The first line of each file is the subject; the rest,
//! after the newline, is the body.

use std::sync::OnceLock;

use sqlx::PgPool;
use tera::Tera;
use vyasa_common::AppError;

/// A rendered email ready to send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedEmail {
    /// Subject line.
    pub subject: String,
    /// Plain-text body.
    pub body: String,
}

/// Every template the system can send, embedded at compile time.
const TEMPLATES: [(&str, &str); 10] = [
    (
        "password_reset",
        include_str!("templates/password_reset.tera"),
    ),
    ("user_invited", include_str!("templates/user_invited.tera")),
    (
        "comment_notification",
        include_str!("templates/comment_notification.tera"),
    ),
    // A plugin supplies its own subject and body; the template exists to
    // give every plugin message the same footer, so a recipient can always
    // tell which plugin wrote to them.
    (
        "plugin_message",
        include_str!("templates/plugin_message.tera"),
    ),
    // Newsletter (phase 61): the double-opt-in confirm, and the one
    // email a new post sends to confirmed subscribers.
    (
        "newsletter_confirm",
        include_str!("templates/newsletter_confirm.tera"),
    ),
    (
        "newsletter_post",
        include_str!("templates/newsletter_post.tera"),
    ),
    // A defined form's answers, to whoever the form names.
    (
        "form_submission",
        include_str!("templates/form_submission.tera"),
    ),
    // Registration (phase 98): the confirmation link; the note an address
    // with an account gets instead; and the set-password link for an
    // account with no password that nobody invited (a confirmed contested
    // registration, or "forgot password" on such an account).
    (
        "registration_confirm",
        include_str!("templates/registration_confirm.tera"),
    ),
    (
        "registration_existing",
        include_str!("templates/registration_existing.tera"),
    ),
    ("password_set", include_str!("templates/password_set.tera")),
];

fn templates() -> &'static Tera {
    static TERA: OnceLock<Tera> = OnceLock::new();
    TERA.get_or_init(|| {
        let mut tera = Tera::default();
        // Emails are plain text: escaping would turn an apostrophe in a
        // post title into `&#x27;` in someone's inbox.
        tera.autoescape_on(Vec::<&str>::new());
        for (name, source) in TEMPLATES {
            // A broken template is a build-time mistake, but panicking here
            // would take the whole process down at first send. Skipping it
            // instead turns the fault into an "unknown template" error at
            // the call site, and the test below fails the build anyway.
            if let Err(err) = tera.add_raw_template(name, source) {
                tracing::error!("email template {name} failed to parse: {err}");
            }
        }
        tera
    })
}

/// Renders a template by name with the given context.
///
/// # Errors
/// Returns [`AppError::Internal`] when the template is unknown or the
/// render fails — both are bugs, not user input.
pub fn render_template(template: &str, ctx: &serde_json::Value) -> Result<RenderedEmail, AppError> {
    let tera = templates();
    let ctx = tera::Context::from_serialize(ctx)
        .map_err(|e| AppError::internal_msg(format!("email context: {e}")))?;
    let rendered = tera
        .render(template, &ctx)
        .map_err(|e| AppError::internal_msg(format!("email render: {e}")))?;
    // Subject is the first line; the body is everything after it.
    let (subject, body) = rendered.split_once('\n').unwrap_or((rendered.as_str(), ""));
    Ok(RenderedEmail {
        subject: subject.trim().to_owned(),
        body: body.trim_start_matches('\n').to_owned(),
    })
}

/// Queues transactional email for delivery by the jobs worker.
#[derive(Clone, Debug)]
pub struct EmailService {
    pool: PgPool,
}

impl EmailService {
    /// Wraps a pool.
    #[must_use]
    pub const fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Renders `template` and queues it for `to`.
    ///
    /// Returns the job id. Rendering happens now so a template bug surfaces
    /// at the call site rather than inside a worker an hour later.
    ///
    /// # Errors
    /// [`AppError::Internal`] on render failure, [`AppError::Db`] if the job
    /// cannot be enqueued.
    pub async fn queue(
        &self,
        to: &str,
        template: &str,
        ctx: &serde_json::Value,
    ) -> Result<i64, AppError> {
        let email = render_template(template, ctx)?;
        let id = vyasa_common::next_id_i64();
        // Inserted directly rather than through `vyasa_jobs::queue` because
        // `core` sits below `jobs` in the dependency order; the media
        // service enqueues the same way.
        sqlx::query(
            "INSERT INTO jobs (id, kind, payload, run_at, status) \
             VALUES ($1, 'send_email', $2, now(), 'queued')",
        )
        .bind(id)
        .bind(serde_json::json!({
            "to": to,
            "subject": email.subject,
            "body": email.body,
            "template": template,
        }))
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::db(format!("queue email failed: {e}")))?;
        Ok(id)
    }
}

#[cfg(test)]
mod tests {
    use super::{render_template, TEMPLATES};

    #[test]
    fn subject_is_the_first_line_and_is_not_in_the_body() {
        let out = render_template(
            "password_reset",
            &serde_json::json!({ "site": "Vyasa", "name": "Ada", "url": "https://x.test/r/1" }),
        )
        .expect("renders");
        assert_eq!(out.subject, "Reset your Vyasa password");
        assert!(
            !out.body.starts_with("Reset your"),
            "subject line must be stripped from the body"
        );
        assert!(out.body.contains("https://x.test/r/1"));
        assert!(out.body.starts_with("Hello,"));
    }

    /// The account's display name may have been typed by whoever first
    /// registered the address, who need not own the mailbox: no mail that
    /// can reach an address on a stranger's say-so puts it in front of the
    /// owner.
    #[test]
    fn mail_a_registrant_could_have_caused_carries_no_display_name() {
        let ctx = serde_json::json!({
            "site": "Lantern Club", "name": "Mallory Typed This",
            "url": "https://x.test/admin/reset?token=t",
            "sign_in_url": "https://x.test/admin/login",
            "forgot_url": "https://x.test/admin/forgot",
            "password_cleared": false,
        });
        for template in [
            "password_reset",
            "registration_existing",
            "registration_confirm",
            "password_set",
        ] {
            let out = render_template(template, &ctx).expect("renders");
            assert!(
                !out.body.contains("Mallory") && !out.subject.contains("Mallory"),
                "{template}: {}",
                out.body
            );
        }
    }

    #[test]
    fn plain_text_bodies_are_not_html_escaped() {
        // Autoescape would render this apostrophe as `&#x27;`.
        let out = render_template(
            "comment_notification",
            &serde_json::json!({
                "name": "Ada",
                "post_title": "Ada's post",
                "author": "Grace",
                "comment_excerpt": "5 > 3 & true",
                "url": "https://x.test/p/1",
            }),
        )
        .expect("renders");
        assert!(out.subject.contains("Ada's post"), "got {:?}", out.subject);
        assert!(out.body.contains("5 > 3 & true"), "got {:?}", out.body);
    }

    #[test]
    fn a_confirmation_names_the_site_and_the_person_and_says_what_to_do_if_unasked() {
        let ctx = |cleared: bool| {
            serde_json::json!({
                "site": "Lantern Club", "name": "Nia",
                "url": "https://x.test/admin/verify?token=t", "password_cleared": cleared,
            })
        };
        let out = render_template("registration_confirm", &ctx(false)).expect("renders");
        assert!(out.subject.contains("Lantern Club"), "{}", out.subject);
        // The name was typed by whoever registered, who may not own the
        // mailbox: it is not put in front of the owner.
        assert!(!out.body.contains("Nia"), "{}", out.body);
        assert!(out.body.contains("https://x.test/admin/verify?token=t"));
        assert!(
            out.body.contains(
                "If you did not create an account, ignore this email — do not click the link."
            ),
            "{}",
            out.body
        );
        assert!(!out.body.contains("set a password"), "{}", out.body);
        // The page asks for the password; without it, none is kept.
        assert!(
            out.body
                .contains("The page asks for the password you chose when you signed up"),
            "{}",
            out.body
        );
        let cleared = render_template("registration_confirm", &ctx(true)).expect("renders");
        assert!(cleared.body.contains("set a password"), "{}", cleared.body);
        assert!(
            cleared.body.contains("confirm without a password"),
            "{}",
            cleared.body
        );
        let set = render_template("password_set", &ctx(false)).expect("renders");
        assert!(!set.body.contains("invited"), "{}", set.body);
        assert!(!set.body.contains("Nia"), "{}", set.body);
    }

    #[test]
    fn every_embedded_template_renders_with_its_documented_context() {
        // Guards against a template gaining a variable that no call site
        // supplies, which would only fail at send time.
        let ctx = serde_json::json!({
            "site": "Vyasa", "name": "Ada", "url": "https://x.test",
            "inviter": "Grace", "role": "editor", "username": "ada",
            "post_title": "Post", "author": "Grace", "comment_excerpt": "hi",
            "plugin": "bookshelf", "subject": "Hello", "body": "A message.",
            "excerpt": "One line about the post.",
            "unsubscribe_url": "https://x.test/newsletter/unsubscribe?token=t",
            "form": "Contact us", "path": "/contact",
            "fields": [{"label": "Email", "value": "ada@example.com"}],
            "password_cleared": true,
            "sign_in_url": "https://x.test/admin/login",
            "forgot_url": "https://x.test/admin/forgot",
        });
        for (name, _) in TEMPLATES {
            let out = render_template(name, &ctx).expect("template renders");
            assert!(!out.subject.is_empty(), "{name} has an empty subject");
            assert!(!out.body.is_empty(), "{name} has an empty body");
        }
    }

    #[test]
    fn unknown_template_is_an_error_not_a_silent_empty_email() {
        assert!(render_template("no_such_template", &serde_json::json!({})).is_err());
    }
}
