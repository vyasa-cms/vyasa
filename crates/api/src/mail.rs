//! The mail relay, configurable from the admin panel as well as the
//! environment.
//!
//! Site options hold host, port, username, from address, and the
//! password sealed with the server secret (the same vault that seals AI
//! provider keys). When the options name a host they win; otherwise the
//! `VYASA_SMTP__*` environment applies; otherwise mail is logged, not
//! sent. The queue's `send_email` handler resolves this per job, so a
//! change in Settings takes effect on the next message with no restart.

use serde::{Deserialize, Serialize};
use vyasa_common::{AppError, Secret, SmtpConfig};
use vyasa_core::ai_models::KeyVault;
use vyasa_jobs::JobHandler;

use crate::state::AppState;

const KEYS: [&str; 5] = [
    "smtp_host",
    "smtp_port",
    "smtp_username",
    "smtp_password",
    "smtp_from",
];

fn vault(state: &AppState) -> KeyVault {
    KeyVault::new(
        state
            .config
            .secret_key
            .as_ref()
            .map(|k| k.expose().as_bytes()),
    )
}

async fn option_string(state: &AppState, key: &str) -> String {
    state
        .options
        .get(key)
        .await
        .ok()
        .and_then(|v| match v {
            serde_json::Value::String(s) => Some(s),
            serde_json::Value::Number(n) => Some(n.to_string()),
            _ => None,
        })
        .unwrap_or_default()
}

/// Where the effective relay comes from.
#[derive(Serialize, utoipa::ToSchema, PartialEq, Eq, Debug, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// Set in the admin panel.
    Options,
    /// Set with `VYASA_SMTP__*`.
    Environment,
    /// Nothing configured; messages are logged.
    None,
}

/// The relay as the admin panel shows it: everything but the password.
#[derive(Serialize, utoipa::ToSchema)]
pub struct MailSettings {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub from: String,
    /// A password is stored (it is never returned).
    pub has_password: bool,
    pub source: Source,
    /// The server secret is set, so the stored password is encrypted.
    pub encrypted: bool,
}

/// What the admin panel saves. A missing `password` keeps the stored
/// one; an empty `host` clears the relay entirely.
#[derive(Deserialize, utoipa::ToSchema, Default)]
pub struct MailInput {
    pub host: String,
    pub port: Option<u16>,
    pub username: Option<String>,
    pub password: Option<String>,
    pub from: String,
}

/// The relay that will actually be used right now.
pub async fn effective(state: &AppState) -> Option<SmtpConfig> {
    let host = option_string(state, "smtp_host").await;
    if host.trim().is_empty() {
        return state.config.smtp.clone();
    }
    let port = option_string(state, "smtp_port")
        .await
        .parse()
        .unwrap_or(587);
    let username = Some(option_string(state, "smtp_username").await).filter(|u| !u.is_empty());
    let sealed = option_string(state, "smtp_password").await;
    let password = if sealed.is_empty() {
        None
    } else {
        vault(state).open(&sealed).ok().map(Secret::new)
    };
    let from = option_string(state, "smtp_from").await;
    Some(SmtpConfig {
        host: host.trim().to_owned(),
        port,
        username,
        password,
        from: if from.is_empty() {
            format!("vyasa@{}", host.trim())
        } else {
            from
        },
    })
}

/// What Settings and the wizard show.
pub async fn settings(state: &AppState) -> MailSettings {
    let host = option_string(state, "smtp_host").await;
    let encrypted = vault(state).encrypting();
    if !host.trim().is_empty() {
        return MailSettings {
            host,
            port: option_string(state, "smtp_port")
                .await
                .parse()
                .unwrap_or(587),
            username: option_string(state, "smtp_username").await,
            from: option_string(state, "smtp_from").await,
            has_password: !option_string(state, "smtp_password").await.is_empty(),
            source: Source::Options,
            encrypted,
        };
    }
    match &state.config.smtp {
        Some(env) => MailSettings {
            host: env.host.clone(),
            port: env.port,
            username: env.username.clone().unwrap_or_default(),
            from: env.from.clone(),
            has_password: env.password.is_some(),
            source: Source::Environment,
            encrypted,
        },
        None => MailSettings {
            host: String::new(),
            port: 587,
            username: String::new(),
            from: String::new(),
            has_password: false,
            source: Source::None,
            encrypted,
        },
    }
}

/// Writes the relay to site options.
///
/// # Errors
/// [`AppError::Validation`] on a from address without `@`, or a port of 0.
pub async fn save(state: &AppState, input: MailInput) -> Result<MailSettings, AppError> {
    let host = input.host.trim().to_owned();
    if host.is_empty() {
        for key in KEYS {
            state
                .options_service
                .put(key, serde_json::Value::String(String::new()))
                .await?;
        }
        return Ok(settings(state).await);
    }
    if host.contains(char::is_whitespace) || host.contains('/') {
        return Err(AppError::validation(
            "the relay host is a hostname, not a URL",
        ));
    }
    let from = input.from.trim().to_owned();
    if !from.contains('@') || from.contains(char::is_whitespace) {
        return Err(AppError::validation(
            "the from address must be an email address the relay is allowed to send as",
        ));
    }
    let port = input.port.unwrap_or(587);
    if port == 0 {
        return Err(AppError::validation("port must be 1..=65535"));
    }
    let put = |k: &'static str, v: String| async move {
        state
            .options_service
            .put(k, serde_json::Value::String(v))
            .await
    };
    put("smtp_host", host).await?;
    put("smtp_port", port.to_string()).await?;
    put(
        "smtp_username",
        input.username.unwrap_or_default().trim().to_owned(),
    )
    .await?;
    put("smtp_from", from).await?;
    if let Some(password) = input.password {
        let sealed = if password.is_empty() {
            String::new()
        } else {
            vault(state).seal(&password)?
        };
        put("smtp_password", sealed).await?;
    }
    Ok(settings(state).await)
}

/// Sends one message through the effective relay, right now.
///
/// # Errors
/// [`AppError::Validation`] with no relay; the relay's reason on failure.
pub async fn send_test(state: &AppState, to: &str) -> Result<(), AppError> {
    let Some(smtp) = effective(state).await else {
        return Err(AppError::validation("no mail relay is configured"));
    };
    let payload = serde_json::json!({
        "to": to,
        "subject": "Vyasa can send mail",
        "body": "This message was sent from Vyasa's mail settings. If you are reading it, password resets, notifications and the newsletter will arrive.",
        "template": "mail-test",
    });
    vyasa_jobs::workers::email::handle(Some(&smtp), payload)
        .await
        .map_err(|e| AppError::external("smtp", e))
}

/// The queue handler for `send_email`: resolves the relay per job so a
/// change in Settings applies without a restart.
pub struct MailJobs(pub AppState);

impl JobHandler for MailJobs {
    fn handle<'a>(
        &'a self,
        kind: &'a str,
        payload: serde_json::Value,
    ) -> vyasa_jobs::HandlerFuture<'a> {
        Box::pin(async move {
            if kind != "send_email" {
                return None;
            }
            let smtp = effective(&self.0).await;
            Some(vyasa_jobs::workers::email::handle(smtp.as_ref(), payload).await)
        })
    }
}
