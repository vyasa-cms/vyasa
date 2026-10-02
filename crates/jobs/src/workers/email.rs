//! Email delivery worker (SMTP via lettre).
//!
//! Sending lives here rather than in `core` so the domain layer stays free
//! of network IO, and so a failed send inherits the queue's backoff and
//! dead-lettering.
//!
//! With no SMTP relay configured the worker logs the message and reports
//! success. That is the dev and test default on purpose: an unconfigured
//! development box must never make an outbound connection, and a queue full
//! of permanently-retrying jobs would bury real failures.

use lettre::message::Mailbox;
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};
use vyasa_common::SmtpConfig;

/// What the email service puts on the queue.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct EmailJob {
    /// Recipient address.
    pub to: String,
    /// Rendered subject line.
    pub subject: String,
    /// Rendered plain-text body.
    pub body: String,
    /// Template name, for logging only.
    #[serde(default)]
    pub template: String,
}

/// Sends one queued email.
///
/// # Errors
/// Returns the failure reason when the address is unusable or the relay
/// rejects the message; the queue retries and eventually dead-letters.
pub async fn handle(smtp: Option<&SmtpConfig>, payload: serde_json::Value) -> Result<(), String> {
    let job: EmailJob =
        serde_json::from_value(payload).map_err(|e| format!("bad email payload: {e}"))?;

    let Some(smtp) = smtp else {
        tracing::info!(
            to = %job.to,
            template = %job.template,
            subject = %job.subject,
            "email not sent: no SMTP relay configured (log-only mode)"
        );
        return Ok(());
    };

    let from: Mailbox = smtp
        .from
        .parse()
        .map_err(|e| format!("invalid envelope-from {:?}: {e}", smtp.from))?;
    // A malformed recipient will never become valid, so this must not be
    // retried five times before dying — but the queue has no notion of a
    // permanent failure, so it is at least reported precisely.
    let to: Mailbox = job
        .to
        .parse()
        .map_err(|e| format!("invalid recipient {:?}: {e}", job.to))?;

    let message = Message::builder()
        .from(from)
        .to(to)
        .subject(&job.subject)
        .header(lettre::message::header::ContentType::TEXT_PLAIN)
        .body(job.body.clone())
        .map_err(|e| format!("build message: {e}"))?;

    let mut builder = AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&smtp.host)
        .map_err(|e| format!("smtp relay {}: {e}", smtp.host))?
        .port(smtp.port);
    if let (Some(user), Some(pass)) = (smtp.username.as_ref(), smtp.password.as_ref()) {
        builder = builder.credentials(Credentials::new(user.clone(), pass.expose().to_owned()));
    }
    let transport = builder.build();

    transport
        .send(message)
        .await
        .map_err(|e| format!("smtp send to {}: {e}", job.to))?;
    tracing::info!(to = %job.to, template = %job.template, "email sent");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{handle, EmailJob};

    fn job() -> serde_json::Value {
        serde_json::to_value(EmailJob {
            to: String::from("ada@example.test"),
            subject: String::from("Hello"),
            body: String::from("Body"),
            template: String::from("password_reset"),
        })
        .expect("encode")
    }

    #[tokio::test]
    async fn no_relay_configured_logs_and_succeeds() {
        // The dev default must not attempt a connection, and must not leave
        // the job retrying forever.
        assert!(handle(None, job()).await.is_ok());
    }

    #[tokio::test]
    async fn malformed_payload_is_reported() {
        let err = handle(None, serde_json::json!({ "to": 42 }))
            .await
            .expect_err("must reject");
        assert!(err.contains("bad email payload"), "got {err}");
    }

    #[tokio::test]
    async fn invalid_recipient_is_rejected_before_connecting() {
        let smtp = vyasa_common::SmtpConfig::default();
        let mut bad = job();
        bad["to"] = serde_json::json!("not an address");
        let err = handle(Some(&smtp), bad).await.expect_err("must reject");
        assert!(err.contains("invalid recipient"), "got {err}");
    }
}
