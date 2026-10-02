//! Allowlisted iframe embed resolution.
//!
//! Only known providers produce an `<iframe>` source; every other URL falls
//! back to a plain link. All extracted path/query components are validated
//! against strict charsets before being reassembled, so nothing user-
//! controlled reaches attribute position unescaped.

/// Outcome of resolving a candidate embed URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmbedOutcome {
    /// A safe iframe URL for an allowlisted provider.
    Iframe {
        /// Provider name (used as CSS hook).
        provider: &'static str,
        /// The reassembled, validated embed URL.
        src: String,
    },
    /// Not an allowlisted provider; render a normal link instead.
    Link,
}

fn valid_token(token: &str, max: usize) -> bool {
    !token.is_empty()
        && token.len() <= max
        && token
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

fn valid_numeric_id(token: &str) -> bool {
    !token.is_empty() && token.len() <= 24 && token.chars().all(|c| c.is_ascii_digit())
}

/// Splits `https://host/path?query` into parts without a URL crate.
struct ParsedUrl<'a> {
    host: &'a str,
    path: Vec<&'a str>,
    query: Option<&'a str>,
}

fn parse_url(raw: &str) -> Option<ParsedUrl<'_>> {
    let rest = raw.strip_prefix("https://")?;
    let (authority, tail) = rest.split_once('/')?;
    let host = authority.split('@').next()?.split(':').next()?;
    if host.is_empty() || !valid_host(host) {
        return None;
    }
    let (path, query) = match tail.split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (tail, None),
    };
    let path: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    Some(ParsedUrl { host, path, query })
}

fn valid_host(host: &str) -> bool {
    host.chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-'))
        && !host.contains("..")
}

fn query_param<'a>(query: Option<&'a str>, key: &str) -> Option<&'a str> {
    let q = query?;
    q.split('&')
        .find_map(|pair| pair.split_once('='))
        .filter(|(k, _)| *k == key)
        .map(|(_, v)| v)
}

const YT_HOSTS: [&str; 3] = ["youtube.com", "www.youtube.com", "m.youtube.com"];

fn youtube_id(query: Option<&str>, segs: &[&str]) -> Option<String> {
    if segs.first() == Some(&"watch") {
        return query_param(query, "v")
            .filter(|i| valid_token(i, 32))
            .map(str::to_owned);
    }
    if segs.len() == 2 && matches!(segs[0], "embed" | "shorts") {
        return Some(segs[1])
            .filter(|i| valid_token(i, 32))
            .map(str::to_owned);
    }
    None
}

fn spotify_ok(segs: &[&str]) -> bool {
    let shaped = if segs.first() == Some(&"embed") {
        segs.len() == 3
    } else {
        segs.len() == 2
            && matches!(
                segs[0],
                "track" | "album" | "playlist" | "artist" | "episode" | "show"
            )
    };
    shaped
        && segs
            .iter()
            .all(|s| s.len() <= 40 && s.chars().all(|c| c.is_ascii_alphanumeric()))
}

/// SoundCloud embeds hand the validated path to the official player.
fn soundcloud(segs: &[&str]) -> Result<EmbedOutcome, ()> {
    if segs.len() < 2 || !segs.iter().all(|s| valid_token(s, 60)) {
        return Err(());
    }
    Ok(EmbedOutcome::Iframe {
        provider: "soundcloud",
        src: format!(
            "https://w.soundcloud.com/player/?url=https%3A%2F%2Fsoundcloud.com%2F{}",
            segs.join("/")
        ),
    })
}

/// Resolves a raw URL to an embed outcome.
///
/// Returns [`EmbedOutcome::Link`] for anything not exactly matching an
/// allowlisted provider pattern (including `http://` URLs — https only).
#[must_use]
pub fn resolve_embed(raw_url: &str) -> EmbedOutcome {
    let Some(parsed) = parse_url(raw_url) else {
        return EmbedOutcome::Link;
    };
    let host = parsed.host;
    let segs = &parsed.path;

    let outcome = if YT_HOSTS.contains(&host) || host == "youtu.be" {
        let id = if host == "youtu.be" {
            segs.first()
                .copied()
                .filter(|i| valid_token(i, 32))
                .map(str::to_owned)
        } else {
            youtube_id(parsed.query, segs)
        };
        id.map_or(EmbedOutcome::Link, |id| EmbedOutcome::Iframe {
            provider: "youtube",
            src: format!("https://www.youtube-nocookie.com/embed/{id}"),
        })
    } else if matches!(host, "vimeo.com" | "player.vimeo.com") {
        let id = match host {
            "player.vimeo.com" => segs.last().copied(),
            _ => segs.first().copied(),
        }
        .filter(|i| valid_numeric_id(i));
        id.map_or(EmbedOutcome::Link, |id| EmbedOutcome::Iframe {
            provider: "vimeo",
            src: format!("https://player.vimeo.com/video/{id}"),
        })
    } else if matches!(host, "dailymotion.com" | "www.dailymotion.com" | "dai.ly") {
        let id = if host == "dai.ly" {
            segs.first().copied()
        } else if segs.len() >= 2 && segs[0] == "video" {
            Some(segs[1])
        } else {
            None
        }
        .filter(|i| valid_token(i, 32));
        id.map_or(EmbedOutcome::Link, |id| EmbedOutcome::Iframe {
            provider: "dailymotion",
            src: format!("https://www.dailymotion.com/embed/video/{id}"),
        })
    } else if host == "open.spotify.com" {
        if !spotify_ok(segs) {
            return EmbedOutcome::Link;
        }
        EmbedOutcome::Iframe {
            provider: "spotify",
            src: format!("https://open.spotify.com/embed/{}", segs.join("/")),
        }
    } else if matches!(host, "soundcloud.com" | "www.soundcloud.com") {
        match soundcloud(segs) {
            Ok(outcome) => outcome,
            Err(()) => return EmbedOutcome::Link,
        }
    } else if matches!(host, "ted.com" | "www.ted.com") {
        if segs.len() != 2 || segs[0] != "talks" || !valid_token(segs[1], 80) {
            return EmbedOutcome::Link;
        }
        EmbedOutcome::Iframe {
            provider: "ted",
            src: format!("https://embed.ted.com/talks/{}", segs[1]),
        }
    } else if host == "archive.org" {
        if segs.len() != 2 || segs[0] != "details" || !valid_token(segs[1], 100) {
            return EmbedOutcome::Link;
        }
        EmbedOutcome::Iframe {
            provider: "archive-org",
            src: format!("https://archive.org/embed/{}", segs[1]),
        }
    } else if host == "speakerdeck.com" {
        if segs.len() != 2
            || !valid_token(segs[0], 40)
            || !segs[1].chars().all(|c| c.is_ascii_alphanumeric())
        {
            return EmbedOutcome::Link;
        }
        EmbedOutcome::Iframe {
            provider: "speakerdeck",
            src: format!("https://speakerdeck.com/player/{}", segs[1]),
        }
    } else if host == "codepen.io" {
        if segs.len() != 3
            || segs[1] != "pen"
            || !valid_token(segs[0], 40)
            || !valid_token(segs[2], 40)
        {
            return EmbedOutcome::Link;
        }
        EmbedOutcome::Iframe {
            provider: "codepen",
            src: format!("https://codepen.io/{}/embed/{}", segs[0], segs[2]),
        }
    } else {
        // slideshare.net and all unknown hosts fall back to a plain link.
        EmbedOutcome::Link
    };
    outcome
}
