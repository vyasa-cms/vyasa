//! Options domain rules: validation grammar for permalinks and custom CSS,
//! typed accessors over [`OptionsRepo`], and in-process caching keyed to
//! the [`OptionChanged`] invalidation event.

use std::fmt::Write as _;

use serde::{Deserialize, Serialize};
use vyasa_common::AppError;
use vyasa_db::repo::OptionsRepo;

use crate::user::registration;

/// Every option key owned by this service (public subset is a subset of
/// these).
pub const SITE_OPTION_KEYS: &[&str] = &[
    "site_title",
    "site_tagline",
    "site_url",
    "site_logo_media_id",
    "site_favicon_media_id",
    "timezone",
    "date_format",
    "permalink_pattern",
    "posts_per_page",
    "edge_cache_seconds",
    "ai_month_cap_usd",
    "comment_moderation",
    "custom_css",
    // Durable context for the assistants: who the site is for and how it
    // should sound, so generation stops reaching for the median website.
    "brand_kit",
    // AI integrations, each off until an operator turns it on.
    "ai_alt_text",
    "ai_comment_screening",
    "ai_autofill",
    "ai_embeddings",
    "ai_semantic_search",
    "ai_related_posts",
    "ai_images",
    "ai_transcription",
    "ai_read_aloud",
    // Audience (phase 61): the newsletter is off until an operator
    // turns it on — publishing must never surprise-email anyone.
    "newsletter_enabled",
    // First-run setup (phase 76): how far the wizard got, and the site's
    // language tag for the html element.
    "setup_progress",
    "site_language",
    "media_storage_cap_mb",
    // The mail relay from the admin panel (phase 79); the password is
    // sealed by the API layer before it gets here.
    "smtp_host",
    "smtp_port",
    "smtp_username",
    "smtp_password",
    "smtp_from",
    // Public registration (phase 98): off until an owner turns it on, and
    // the role a self-made account starts with.
    "registration_enabled",
    "registration_default_role",
    // Media storage from the admin panel: the keys are sealed by the API
    // layer before they get here; the migration row is the move job's
    // progress.
    "storage_provider",
    "storage_bucket",
    "storage_region",
    "storage_endpoint",
    "storage_path_style",
    "storage_access_key_id",
    "storage_secret_access_key",
    "storage_migration",
];

/// Options that decide where the site's mail goes and which address its
/// links carry. Whoever writes one can take the site over (a
/// password-reset link sent through their relay, or to their address), so
/// the API asks for more than `manage_options` to write them. Where
/// upgrades and packages come from is compiled into the binary.
pub const FULL_ADMINISTRATOR_OPTION_KEYS: &[&str] = &[
    // The absolute address in password-reset and invitation links.
    "site_url",
    // The relay every message, reset links included, is handed to; the
    // port, credentials and sender address redirect or forge it as well.
    "smtp_host",
    "smtp_port",
    "smtp_username",
    "smtp_password",
    "smtp_from",
    // Whether strangers may make accounts, and what those accounts may do.
    "registration_enabled",
    "registration_default_role",
    // The keys that reach every byte the site serves.
    "storage_provider",
    "storage_bucket",
    "storage_region",
    "storage_endpoint",
    "storage_path_style",
    "storage_access_key_id",
    "storage_secret_access_key",
];

/// Boolean AI feature switches (the screening mode is a string).
pub const AI_BOOL_OPTIONS: &[&str] = &[
    "ai_alt_text",
    "ai_autofill",
    "ai_embeddings",
    "ai_semantic_search",
    "ai_related_posts",
    "ai_images",
    "ai_transcription",
    "ai_read_aloud",
];

/// Option keys safe to expose without authentication.
pub const PUBLIC_OPTION_KEYS: &[&str] = &[
    "site_title",
    "site_tagline",
    "site_url",
    "permalink_pattern",
    "posts_per_page",
    "timezone",
    "date_format",
];

/// Permalink pattern: literal text plus `{tokens}`.
///
/// Allowed tokens: `{year}`, `{month}`, `{slug}`, `{type}`. Everything else
/// must be URL-safe literal text (`[a-z0-9/_-]`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermalinkPattern(pub String);

impl PermalinkPattern {
    /// Default pattern used when unset.
    pub const DEFAULT: &'static str = "/post/{slug}";

    /// Validates and wraps a pattern.
    ///
    /// # Errors
    /// [`AppError::Validation`] on unknown tokens or unsafe literals.
    pub fn parse(raw: &str) -> Result<Self, AppError> {
        if !raw.starts_with('/') || raw.len() > 200 || raw.ends_with('/') || raw.contains("//") {
            return Err(AppError::validation(
                "permalink must be a rooted path without empty segments",
            ));
        }
        let mut seen = std::collections::HashSet::new();
        for segment in raw[1..].split('/') {
            if let Some(token) = segment.strip_prefix('{').and_then(|s| s.strip_suffix('}')) {
                if !matches!(token, "year" | "month" | "slug" | "type") || !seen.insert(token) {
                    return Err(AppError::validation(
                        "permalink tokens must be unique: year, month, slug, type",
                    ));
                }
            } else {
                Self::check_literal(segment)?;
            }
        }
        if !seen.contains("slug") {
            return Err(AppError::validation(
                "permalink must contain a {slug} segment",
            ));
        }
        if matches!(
            raw.split('/').nth(1),
            Some(
                "api"
                    | "admin"
                    | "setup"
                    | "login"
                    | "preview"
                    | "theme-assets"
                    | "plugin-assets"
                    | "category"
                    | "tag"
                    | "author"
                    | "archive"
                    | "newsletter"
            )
        ) {
            return Err(AppError::validation("permalink prefix is reserved"));
        }
        Ok(Self(raw.to_owned()))
    }

    /// Canonical path; pages and plugin types keep their own namespaces.
    #[must_use]
    pub fn path_for(&self, post: &vyasa_db::content_models::PostRow) -> String {
        use chrono::Datelike as _;
        use vyasa_db::content_models::PostType;
        match post.post_type {
            PostType::Page => format!("/{}", post.slug),
            PostType::Custom(t) => format!("/{t}/{}", post.slug),
            _ => {
                let at = post.published_at.unwrap_or(post.created_at);
                self.url_for(
                    post.post_type.as_str(),
                    &post.slug,
                    at.year(),
                    u8::try_from(at.month()).unwrap_or(1),
                )
            }
        }
    }

    /// Extracts a candidate slug; callers must compare the full canonical path.
    #[must_use]
    pub fn slug_from<'a>(&self, path: &'a str) -> Option<&'a str> {
        let parts: Vec<_> = path.strip_prefix('/')?.split('/').collect();
        let pattern: Vec<_> = self.0.strip_prefix('/')?.split('/').collect();
        if parts.len() != pattern.len() {
            return None;
        }
        let mut slug = None;
        for (part, token) in parts.into_iter().zip(pattern) {
            match token {
                "{slug}" if !part.is_empty() => slug = Some(part),
                "{year}" | "{month}" if part.bytes().all(|b| b.is_ascii_digit()) => {}
                "{type}" if part == "post" => {}
                literal if literal == part => {}
                _ => return None,
            }
        }
        slug
    }

    fn check_literal(chunk: &str) -> Result<(), AppError> {
        if !chunk
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '/' | '-' | '_'))
        {
            return Err(AppError::validation(
                "permalink literals may only use [a-z0-9/_-]",
            ));
        }
        Ok(())
    }

    /// Builds the public URL for a post from this pattern.
    #[must_use]
    pub fn url_for(&self, post_type: &str, slug: &str, year: i32, month: u8) -> String {
        let mut out = self.0.clone();
        for (token, value) in [
            ("{year}", year.to_string()),
            ("{month}", format!("{month:02}")),
            ("{slug}", slug.to_owned()),
            ("{type}", post_type.to_owned()),
        ] {
            out = out.replace(token, &value);
        }
        out
    }
}

/// Sanitized custom CSS payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomCss {
    /// Sanitized stylesheet text (at-rules stripped, urls restricted).
    pub css: String,
}

/// Maximum accepted custom-CSS length before sanitizing.
pub const MAX_CUSTOM_CSS_BYTES: usize = 64 * 1024;

/// Strip anything dangerous from author-supplied CSS:
/// - every at-rule (`@import`, `@charset`, `@media`, nested …) is removed,
/// - `url(...)` only survives when its argument is `https://…`,
/// - `expression(`, `behavior:` and `-moz-binding` are dropped outright.
///
/// Implemented as a lightweight tokenizer over brace depth rather than a
/// full CSS parser: rules are simple enough that an allowlist scanner gives
/// the same guarantees without a dependency (decision recorded in
/// PHASE-27.md).
#[must_use]
pub fn sanitize_custom_css(input: &str) -> String {
    if input.len() > MAX_CUSTOM_CSS_BYTES {
        return String::new();
    }
    let lower = input.to_ascii_lowercase();
    for banned in ["expression(", "behavior:", "-moz-binding", "javascript:"] {
        if lower.contains(banned) {
            return String::new();
        }
    }

    // Pass 1: drop every top-level at-rule (@import "…"; or @media … { … })
    // including its balanced block. Regular rules pass through.
    let mut stripped = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(&ch) = chars.peek() {
        if ch == '@' {
            // Consume until '{' (then balanced block) or ';'.
            let mut depth = 0i32;
            let mut started = false;
            for c in chars.by_ref() {
                match c {
                    '{' => {
                        depth += 1;
                        started = true;
                    }
                    '}' => {
                        depth -= 1;
                        if started && depth == 0 {
                            break;
                        }
                    }
                    ';' if !started => break,
                    _ => {}
                }
            }
            continue;
        }
        // Copy one balanced regular rule / declaration.
        if ch == '{' {
            let mut depth = 0i32;
            for c in chars.by_ref() {
                stripped.push(c);
                match c {
                    '{' => depth += 1,
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
            }
        } else {
            stripped.push(ch);
            chars.next();
        }
    }

    // Pass 2: keep only https url(...) references.
    restrict_urls(&stripped)
}

fn restrict_urls(css: &str) -> String {
    // Keep only url(https://...) / url('https://...') forms.
    let mut result = String::with_capacity(css.len());
    let mut rest = css;
    while let Some(pos) = rest.to_ascii_lowercase().find("url(") {
        result.push_str(&rest[..pos]);
        let after = &rest[pos + 4..];
        match after.find(')') {
            Some(end) => {
                let arg = after[..end].trim().trim_matches('\'').trim_matches('"');
                if arg.starts_with("https://") {
                    let _ = write!(result, "url(\"{arg}\")");
                }
                rest = &after[end + 1..];
            }
            None => break,
        }
    }
    result.push_str(rest);
    result
}

/// Checks that a site URL is an absolute http(s) origin.
///
/// Rejecting a relative or scheme-less value here matters because the
/// result is pasted into emails and feeds, where a bad value produces
/// links that silently go nowhere.
/// Checks a brand kit without needing a service or a database.
///
/// A brand kit is an object of short free-text fields.
///
/// Free text rather than enums on purpose: the point is context a model can
/// use, and "quiet, technical, no exclamation marks" carries more than any
/// list of options would.
///
/// # Errors
/// [`AppError::Validation`] when the value is not an object of known
/// fields holding short text.
pub fn validate_brand_kit(value: &serde_json::Value) -> Result<(), AppError> {
    if value.is_null() {
        return Ok(());
    }
    let Some(object) = value.as_object() else {
        return Err(AppError::validation("brand_kit must be an object"));
    };
    for (key, field) in object {
        if !BRAND_KIT_FIELDS.contains(&key.as_str()) {
            return Err(AppError::validation(format!(
                "brand_kit: unknown field \"{key}\" (expected one of: {})",
                BRAND_KIT_FIELDS.join(", ")
            )));
        }
        let Some(text) = field.as_str() else {
            return Err(AppError::validation(format!(
                "brand_kit.{key} must be text"
            )));
        };
        if text.chars().count() > MAX_BRAND_FIELD {
            return Err(AppError::validation(format!(
                "brand_kit.{key} is longer than {MAX_BRAND_FIELD} characters"
            )));
        }
    }
    Ok(())
}

/// Renders a stored brand kit as the lines a prompt carries.
///
/// Field order comes from [`BRAND_KIT_FIELDS`] rather than the JSON
/// object's, so two sites with the same kit produce the same prompt, and
/// blank fields are dropped — a heading with nothing under it is just
/// something the model has to read past.
#[must_use]
pub fn brand_kit_prompt_of(value: Option<&serde_json::Value>) -> String {
    use std::fmt::Write as _;
    let Some(object) = value.and_then(serde_json::Value::as_object) else {
        return String::new();
    };
    let mut out = String::new();
    for key in BRAND_KIT_FIELDS {
        let text = object
            .get(key)
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        if text.trim().is_empty() {
            continue;
        }
        let _ = writeln!(out, "- {key}: {}", text.trim());
    }
    out
}

fn validate_site_url(raw: &str) -> Result<(), AppError> {
    let rest = raw
        .strip_prefix("https://")
        .or_else(|| raw.strip_prefix("http://"))
        .ok_or_else(|| AppError::validation("site_url must start with http:// or https://"))?;
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    if host.is_empty() {
        return Err(AppError::validation("site_url must include a host"));
    }
    if host.contains(char::is_whitespace) {
        return Err(AppError::validation(
            "site_url host must not contain spaces",
        ));
    }
    Ok(())
}

use crate::events;

/// The most any one brand-kit field may say.
///
/// Long enough for a paragraph of real guidance, short enough that seven of
/// them do not crowd out the actual request in the prompt.
const MAX_BRAND_FIELD: usize = 600;

/// The fields a brand kit may carry, in the order they are shown to a model.
pub const BRAND_KIT_FIELDS: [&str; 6] = [
    "audience",
    "voice",
    "palette",
    "typography",
    "references",
    "avoid",
];

/// Typed options facade: validation on write, cache-friendly reads.
#[derive(Clone, Debug)]
pub struct OptionsService {
    repo: OptionsRepo,
}

impl OptionsService {
    /// Wraps an options repository.
    #[must_use]
    pub fn new(repo: OptionsRepo) -> Self {
        Self { repo }
    }

    async fn raw(&self, key: &str) -> Result<Option<serde_json::Value>, AppError> {
        match self.repo.get(key).await {
            Ok(v) => Ok(Some(v)),
            Err(AppError::NotFound { .. }) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Checks a key/value pair without writing it.
    ///
    /// Split out of [`Self::put`] so a caller writing several options can
    /// validate the whole set first and avoid a partial update.
    ///
    /// # Errors
    /// [`AppError::Validation`] for malformed values or unknown keys.
    pub fn validate(&self, key: &str, value: &serde_json::Value) -> Result<(), AppError> {
        if !SITE_OPTION_KEYS.contains(&key) {
            return Err(AppError::validation(format!("unknown option \"{key}\"")));
        }
        match key {
            "brand_kit" => return validate_brand_kit(value),
            "site_title" | "site_tagline" => {
                let text = value.as_str().unwrap_or("");
                if text.chars().count() > 200 {
                    return Err(AppError::validation("text must be <= 200 chars"));
                }
            }
            "site_url" => {
                let raw = value.as_str().unwrap_or("");
                if !raw.is_empty() {
                    validate_site_url(raw)?;
                }
            }
            "timezone" => {
                if value
                    .as_str()
                    .and_then(|s| s.parse::<chrono_tz::Tz>().ok())
                    .is_none()
                {
                    return Err(AppError::validation(
                        "timezone must be an IANA timezone such as Europe/Stockholm",
                    ));
                }
            }
            "permalink_pattern" => {
                let raw = value.as_str().unwrap_or(Self::DEFAULT_PATTERN);
                PermalinkPattern::parse(raw)?;
            }
            "posts_per_page" => {
                let n = value.as_u64().unwrap_or(10);
                if !(1..=100).contains(&n) {
                    return Err(AppError::validation("posts_per_page must be 1..=100"));
                }
            }
            "ai_month_cap_usd" => {
                // Null unsets it. Otherwise a non-negative amount; zero
                // means "spend nothing", which is a real setting.
                if !value.is_null() && !value.as_f64().is_some_and(|n| n >= 0.0) {
                    return Err(AppError::validation(
                        "ai_month_cap_usd must be a non-negative number, or null to remove the cap",
                    ));
                }
            }
            "edge_cache_seconds" => {
                let n = value.as_u64().unwrap_or(0);
                // A day is the practical ceiling: past that, a purge that
                // fails leaves a page wrong for longer than anyone would
                // tolerate discovering it.
                if n > 86_400 {
                    return Err(AppError::validation(
                        "edge_cache_seconds must be 0..=86400 (0 turns edge caching off)",
                    ));
                }
            }
            // These are the values `ModerationMode::parse` understands.
            // The three this validator used to accept (none/hold_new/all)
            // matched nothing, so every setting an operator saved fell
            // through to the default and the option did nothing at all.
            "comment_moderation"
                if !matches!(
                    value.as_str(),
                    Some("auto_approve" | "require_first" | "require_all")
                ) =>
            {
                return Err(AppError::validation(
                    "comment_moderation must be auto_approve|require_first|require_all",
                ));
            }
            "ai_comment_screening" if !matches!(value.as_str(), Some("off" | "flag" | "spam")) => {
                return Err(AppError::validation(
                    "ai_comment_screening must be off|flag|spam",
                ));
            }
            k if k.starts_with("registration_") => return registration_option(k, value),
            "newsletter_enabled" if !value.is_boolean() => {
                return Err(AppError::validation(
                    "newsletter_enabled must be true or false",
                ));
            }
            k if AI_BOOL_OPTIONS.contains(&k) && !value.is_boolean() => {
                return Err(AppError::validation(format!("{k} must be true or false")));
            }
            _ => {}
        }
        Ok(())
    }

    /// [`Self::validate`], plus the checks that have to read storage:
    /// `registration_default_role` must be a built-in role or a custom
    /// role that exists, and must not hold a capability in
    /// [`registration::FORBIDDEN_DEFAULT_ROLE_CAPS`]. [`Self::put`] runs
    /// this; a caller writing several options runs it for each first.
    ///
    /// # Errors
    /// [`AppError::Validation`] for malformed values, unknown keys, or a
    /// default role that is missing or too powerful; repository errors.
    pub async fn validate_with_lookups(
        &self,
        key: &str,
        value: &serde_json::Value,
    ) -> Result<(), AppError> {
        self.validate(key, value)?;
        if key == "registration_default_role" {
            let name = value.as_str().unwrap_or("");
            if vyasa_db::models::Role::parse(name).is_err() {
                match self.repo.custom_role_capabilities(name).await? {
                    None => {
                        return Err(AppError::validation(format!("there is no role \"{name}\"")));
                    }
                    Some(caps) if !registration::capabilities_may_be_default(&caps) => {
                        return Err(default_role_refused(name));
                    }
                    Some(_) => {}
                }
            }
        }
        Ok(())
    }

    /// Whether visitors may create their own accounts (default: no).
    ///
    /// # Errors
    /// Propagates repository errors.
    pub async fn registration_enabled(&self) -> Result<bool, AppError> {
        Ok(self
            .raw("registration_enabled")
            .await?
            .and_then(|v| v.as_bool())
            .unwrap_or(false))
    }

    /// The role name stored for self-registered accounts (default
    /// `subscriber`). This is what was saved, not what will be used:
    /// registration checks it again when an account is created.
    ///
    /// # Errors
    /// Propagates repository errors.
    pub async fn registration_default_role(&self) -> Result<String, AppError> {
        Ok(self
            .raw("registration_default_role")
            .await?
            .and_then(|v| v.as_str().map(str::to_owned))
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| String::from("subscriber")))
    }

    /// Writes any managed option after per-key validation.
    ///
    /// # Errors
    /// [`AppError::Validation`] for malformed values or unknown keys.
    pub async fn put(&self, key: &str, value: serde_json::Value) -> Result<(), AppError> {
        self.validate_with_lookups(key, &value).await?;
        if key == "permalink_pattern" {
            let old = self.permalink_pattern().await?;
            let new = PermalinkPattern::parse(value.as_str().unwrap_or(Self::DEFAULT_PATTERN))?;
            let posts = self.repo.published_posts().await?;
            let redirects: Vec<_> = posts
                .iter()
                .map(|p| (old.path_for(p), new.path_for(p)))
                .filter(|(a, b)| a != b)
                .collect();
            self.repo
                .set_with_redirects(key, &value, &redirects)
                .await?;
            events::emit_option_changed(key);
            return Ok(());
        }
        // Stored without a trailing slash so every caller can concatenate a
        // rooted path without producing a double slash.
        if key == "site_url" {
            let trimmed = value
                .as_str()
                .unwrap_or("")
                .trim_end_matches('/')
                .to_owned();
            self.repo
                .set(key, &serde_json::Value::String(trimmed))
                .await?;
            events::emit_option_changed(key);
            return Ok(());
        }
        // Custom CSS is stored sanitized rather than as supplied.
        if key == "custom_css" {
            let css = sanitize_custom_css(value.as_str().unwrap_or(""));
            self.repo.set(key, &serde_json::Value::String(css)).await?;
            events::emit_option_changed(key);
            return Ok(());
        }
        self.repo.set(key, &value).await?;
        events::emit_option_changed(key);
        Ok(())
    }

    /// Default permalink pattern constant for callers that need it.
    pub const DEFAULT_PATTERN: &'static str = PermalinkPattern::DEFAULT;

    /// Site identity block (title/tagline/logo/favicon ids).
    ///
    /// # Errors
    /// Propagates repository errors.
    pub async fn site_identity(&self) -> Result<SiteIdentity, AppError> {
        Ok(SiteIdentity {
            title: self.string_of("site_title").await?,
            tagline: self.string_of("site_tagline").await?,
            logo_media_id: self.id_of("site_logo_media_id").await,
            favicon_media_id: self.id_of("site_favicon_media_id").await,
        })
    }

    /// Absolute public base URL with no trailing slash, e.g.
    /// `https://blog.example`.
    ///
    /// Empty when unset. Callers that must emit an absolute URL — feeds,
    /// sitemaps, emails — should fall back to the request's own origin
    /// rather than inventing a host.
    ///
    /// # Errors
    /// Propagates repository errors.
    pub async fn site_url(&self) -> Result<String, AppError> {
        Ok(self
            .string_of("site_url")
            .await?
            .trim_end_matches('/')
            .to_owned())
    }

    /// Effective posts-per-page (default 10).
    ///
    /// # Errors
    /// Propagates repository errors.
    pub async fn posts_per_page(&self) -> Result<i64, AppError> {
        let raw = self.raw("posts_per_page").await?.and_then(|v| v.as_u64());
        match raw {
            Some(n) => Ok(i64::try_from(n).unwrap_or(10).clamp(1, 100)),
            None => Ok(10),
        }
    }

    /// How long a CDN may hold a public page, in seconds.
    ///
    /// Zero — the default — means the edge revalidates every time, which
    /// is the only safe setting for a site with no purge configured.
    ///
    /// # Errors
    /// Propagates repository errors.
    pub async fn edge_cache_seconds(&self) -> Result<u64, AppError> {
        Ok(self
            .raw("edge_cache_seconds")
            .await?
            .and_then(|v| v.as_u64())
            .unwrap_or(0)
            .min(86_400))
    }

    /// Effective permalink pattern (default [`PermalinkPattern::DEFAULT`]).
    ///
    /// # Errors
    /// Propagates repository errors.
    pub async fn permalink_pattern(&self) -> Result<PermalinkPattern, AppError> {
        match self.raw("permalink_pattern").await? {
            Some(v) => v.as_str().map(str::to_owned).map_or_else(
                || PermalinkPattern::parse(Self::DEFAULT_PATTERN),
                |s| PermalinkPattern::parse(&s),
            ),
            None => PermalinkPattern::parse(Self::DEFAULT_PATTERN),
        }
    }

    /// Sanitized custom CSS, empty when unset.
    ///
    /// # Errors
    /// Propagates repository errors.
    pub async fn custom_css(&self) -> Result<String, AppError> {
        Ok(self
            .raw("custom_css")
            .await?
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default())
    }

    /// The brand kit as prose for a prompt, empty when nothing is set.
    ///
    /// Rendered here rather than in the AI crate so both assistants say the
    /// same thing, and so the order does not depend on a JSON object's.
    ///
    /// # Errors
    /// Propagates repository errors.
    pub async fn brand_kit_prompt(&self) -> Result<String, AppError> {
        Ok(brand_kit_prompt_of(self.raw("brand_kit").await?.as_ref()))
    }

    /// All public options as a JSON object (missing keys omitted).
    ///
    /// # Errors
    /// Propagates repository errors.
    pub async fn public_map(&self) -> Result<serde_json::Map<String, serde_json::Value>, AppError> {
        let mut out = serde_json::Map::new();
        for key in PUBLIC_OPTION_KEYS {
            if let Some(v) = self.raw(key).await? {
                out.insert((*key).to_owned(), v);
            }
        }
        Ok(out)
    }

    /// Public typed accessor for a single string option.
    ///
    /// # Errors
    /// Propagates repository errors.
    pub async fn string_option(&self, key: &str) -> Result<String, AppError> {
        self.string_of(key).await
    }

    async fn string_of(&self, key: &str) -> Result<String, AppError> {
        Ok(self
            .raw(key)
            .await?
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default())
    }

    async fn id_of(&self, key: &str) -> Option<i64> {
        let value = self.raw(key).await.ok()??;
        value.as_i64()
    }
}

/// Site identity values as consumed by themes and feeds.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SiteIdentity {
    /// Site title.
    pub title: String,
    /// Tagline.
    pub tagline: String,
    /// Logo media id.
    pub logo_media_id: Option<i64>,
    /// Favicon media id.
    pub favicon_media_id: Option<i64>,
}

/// The shape of the two registration options. A custom role's slug
/// passes here on its shape alone; whether it exists and what it holds
/// is [`OptionsService::validate_with_lookups`].
fn registration_option(key: &str, value: &serde_json::Value) -> Result<(), AppError> {
    if key == "registration_enabled" {
        if !value.is_boolean() {
            return Err(AppError::validation(
                "registration_enabled must be true or false",
            ));
        }
        return Ok(());
    }
    let name = value.as_str().unwrap_or("");
    if name.is_empty() {
        return Err(AppError::validation(
            "registration_default_role must name a role",
        ));
    }
    match vyasa_db::models::Role::parse(name) {
        Ok(role) if !registration::builtin_role_may_be_default(role) => {
            Err(default_role_refused(name))
        }
        _ => Ok(()),
    }
}

fn default_role_refused(name: &str) -> AppError {
    AppError::validation(format!(
        "\"{name}\" cannot be the role new registrations get: it can manage users, \
         options, plugins, themes or categories, edit others' content or moderate comments"
    ))
}
