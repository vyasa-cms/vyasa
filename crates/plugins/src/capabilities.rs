//! Capability grammar: `db:read:posts`, `db:read:options`,
//! `db:read:comments`, `db:write:posts`, `db:publish:posts`,
//! `db:write:meta`, `kv:read`, `kv:write`, `viewer:read`, `mail:send`,
//! `net:fetch:{pattern}`, `log:write`, `event:emit`, `ai:text`,
//! `html:page`, `assets:script`.
//!
//! Patterns use shell-style host wildcards (`*.example.com` matches
//! `a.example.com` and `a.b.example.com` but never `example.com` itself
//! nor `evil-example.com`).

use std::fmt;

/// A declared or requested capability.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Capability {
    /// Read access to post content.
    DbReadPosts,
    /// Read access to public options.
    DbReadOptions,
    /// Read access to approved comments.
    DbReadComments,
    /// Create and edit posts the plugin itself created.
    ///
    /// Deliberately not "edit any post": a plugin holding this may not
    /// touch content a person wrote. See `plugin_posts`.
    DbWritePosts,
    /// Publish, on top of [`Capability::DbWritePosts`].
    ///
    /// Separate because the difference between a draft waiting for review
    /// and text live on the site is the whole reason an operator reads the
    /// capability list before installing anything.
    DbPublishPosts,
    /// Read the plugin's own private key/value namespace.
    KvRead,
    /// Write the plugin's own private key/value namespace.
    KvWrite,
    /// Write the plugin's own namespaced meta on a post.
    DbWriteMeta,
    /// See who is signed in.
    ViewerRead,
    /// Queue mail to an address the site already knows.
    MailSend,
    /// Outbound fetch restricted to hosts matching the pattern.
    NetFetch(String),
    /// Write structured logs.
    LogWrite,
    /// Emit domain events.
    EventEmit,
    /// Ask the site's text model for a completion.
    AiText,
    /// Rewrite the fully rendered HTML of every public page (the
    /// `page-html` filter stage).
    ///
    /// Its own line because it is the power to put anything — script
    /// included — in front of every visitor; being installed is not it.
    HtmlPage,
    /// Ship a JavaScript file that every public page loads.
    ///
    /// Stylesheets stay ungated: CSS cannot run code. A script can, so
    /// declaring one is not enough — the operator has to have granted it.
    AssetsScript,
}

impl fmt::Display for Capability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DbReadPosts => write!(f, "db:read:posts"),
            Self::DbReadOptions => write!(f, "db:read:options"),
            Self::DbReadComments => write!(f, "db:read:comments"),
            Self::DbWritePosts => write!(f, "db:write:posts"),
            Self::DbPublishPosts => write!(f, "db:publish:posts"),
            Self::KvRead => write!(f, "kv:read"),
            Self::KvWrite => write!(f, "kv:write"),
            Self::DbWriteMeta => write!(f, "db:write:meta"),
            Self::ViewerRead => write!(f, "viewer:read"),
            Self::MailSend => write!(f, "mail:send"),
            Self::NetFetch(p) => write!(f, "net:fetch:{p}"),
            Self::LogWrite => write!(f, "log:write"),
            Self::EventEmit => write!(f, "event:emit"),
            Self::AiText => write!(f, "ai:text"),
            Self::HtmlPage => write!(f, "html:page"),
            Self::AssetsScript => write!(f, "assets:script"),
        }
    }
}

impl Capability {
    /// Parses one capability string.
    ///
    /// # Errors
    /// Returns the raw string as an error message when unrecognised.
    pub fn parse(raw: &str) -> Result<Self, String> {
        match raw {
            "db:read:posts" => Ok(Self::DbReadPosts),
            "db:read:options" => Ok(Self::DbReadOptions),
            "db:read:comments" => Ok(Self::DbReadComments),
            "db:write:posts" => Ok(Self::DbWritePosts),
            "db:publish:posts" => Ok(Self::DbPublishPosts),
            "kv:read" => Ok(Self::KvRead),
            "kv:write" => Ok(Self::KvWrite),
            "db:write:meta" => Ok(Self::DbWriteMeta),
            "viewer:read" => Ok(Self::ViewerRead),
            "mail:send" => Ok(Self::MailSend),
            "log:write" => Ok(Self::LogWrite),
            "event:emit" => Ok(Self::EventEmit),
            "ai:text" => Ok(Self::AiText),
            "html:page" => Ok(Self::HtmlPage),
            "assets:script" => Ok(Self::AssetsScript),
            other => {
                if let Some(pattern) = other.strip_prefix("net:fetch:") {
                    if pattern.is_empty() || pattern.contains('/') {
                        return Err(format!("invalid net:fetch pattern {pattern:?}"));
                    }
                    return Ok(Self::NetFetch(pattern.to_owned()));
                }
                Err(format!("unknown capability {raw:?}"))
            }
        }
    }

    /// Parses a manifest JSON array of capability strings.
    ///
    /// # Errors
    /// Returns the first invalid entry.
    pub fn parse_list(raw: &serde_json::Value) -> Result<Vec<Self>, String> {
        let arr = raw.as_array().ok_or("capabilities must be an array")?;
        arr.iter()
            .filter_map(serde_json::Value::as_str)
            .map(Self::parse)
            .collect()
    }

    /// Whether this capability satisfies a request for `wanted`
    /// (exact match, or net:fetch wildcard covering the wanted host).
    #[must_use]
    pub fn implies(&self, wanted: &Capability) -> bool {
        match (self, wanted) {
            (Self::NetFetch(granted), Self::NetFetch(host)) => host_matches(granted, host),
            // Writing a namespace you cannot read back is not a thing
            // anyone wants; granting write grants read of the same
            // (plugin-private) keys.
            (Self::KvWrite, Self::KvRead) => true,
            _ => self == wanted,
        }
    }
}

/// Wildcard host match. `*.example.com` matches exactly one leading
/// label group (`a.example.com`, `b.example.com`) but never the bare
/// domain, deeper subdomains, or look-alike suffixes (`evil-example.com`).
#[must_use]
pub fn host_matches(pattern: &str, host: &str) -> bool {
    let (pat, host) = (pattern.to_ascii_lowercase(), host.to_ascii_lowercase());
    if let Some(rest) = pat.strip_prefix("*.") {
        // Host must be <one-label>.<rest>.
        let Some(stripped) = host.strip_suffix(&format!(".{rest}")) else {
            return false;
        };
        !stripped.is_empty() && !stripped.contains('.') && !stripped.starts_with('-')
    } else {
        pat == host
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_roundtrip() {
        assert_eq!(
            Capability::parse("db:read:posts").unwrap(),
            Capability::DbReadPosts
        );
        assert_eq!(
            Capability::parse("net:fetch:*.example.com").unwrap(),
            Capability::NetFetch(String::from("*.example.com"))
        );
        assert_eq!(Capability::parse("kv:write").unwrap(), Capability::KvWrite);
        assert_eq!(
            Capability::parse("db:publish:posts").unwrap(),
            Capability::DbPublishPosts
        );
        assert_eq!(
            Capability::parse("html:page").unwrap(),
            Capability::HtmlPage
        );
        assert_eq!(
            Capability::parse("assets:script").unwrap(),
            Capability::AssetsScript
        );
        for cap in [Capability::HtmlPage, Capability::AssetsScript] {
            assert_eq!(Capability::parse(&cap.to_string()).unwrap(), cap);
        }
        for bad in [
            "db:write",
            "net:fetch:",
            "net:fetch:a/b",
            "",
            "log",
            "kv",
            "html",
            "assets",
        ] {
            assert!(Capability::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn parse_list_rejects_unknown() {
        let v = serde_json::json!(["log:write", "bogus"]);
        assert!(Capability::parse_list(&v).is_err());
        let v = serde_json::json!(["log:write"]);
        assert!(Capability::parse_list(&v).is_ok());
    }

    #[test]
    fn implies_exact_and_net() {
        let granted = [Capability::DbReadPosts, Capability::LogWrite];
        let g = |c| granted.iter().any(|g| g.implies(&c));
        assert!(g(Capability::DbReadPosts));
        assert!(!g(Capability::EventEmit));
        let net = Capability::NetFetch(String::from("*.example.com"));
        assert!(net.implies(&Capability::NetFetch(String::from("a.example.com"))));
        assert!(net.implies(&Capability::NetFetch(String::from("b.example.com"))));
        assert!(!net.implies(&Capability::NetFetch(String::from("example.com"))));
        assert!(!net.implies(&Capability::NetFetch(String::from("evil.com"))));
        assert!(!net.implies(&Capability::NetFetch(String::from("a.b.example.com"))));
        // Exact-host grant matches itself.
        let exact = Capability::NetFetch(String::from("api.example.com"));
        assert!(exact.implies(&exact));
    }

    #[test]
    fn write_implies_read_but_publish_stands_alone() {
        assert!(Capability::KvWrite.implies(&Capability::KvRead));
        assert!(!Capability::KvRead.implies(&Capability::KvWrite));
        // The point of splitting these: a plugin that may write drafts
        // must not thereby be able to publish them.
        assert!(!Capability::DbWritePosts.implies(&Capability::DbPublishPosts));
        // Nor does writing posts imply writing meta, seeing viewers or
        // sending mail: each is a separate line in the install dialog.
        for other in [
            Capability::DbWriteMeta,
            Capability::ViewerRead,
            Capability::MailSend,
        ] {
            assert!(!Capability::DbWritePosts.implies(&other), "{other}");
        }
    }

    #[test]
    fn wildcard_never_matches_lookalike() {
        let net = Capability::NetFetch(String::from("*.example.com"));
        assert!(!net.implies(&Capability::NetFetch(String::from("evil-example.com"))));
    }
}
