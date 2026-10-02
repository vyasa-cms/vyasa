//! sitemap.xml + robots.txt for the public site.

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};

use crate::state::AppState;

/// The site's absolute base URL.
///
/// Both documents here are consumed by machines that follow the URLs they
/// contain, so a wrong host is not a cosmetic problem: the sitemap and
/// robots.txt previously hardcoded `http://localhost:8080`, which told every
/// crawler that the whole site lived on their own loopback address.
async fn base_url(state: &AppState, headers: &HeaderMap) -> String {
    match state.options_service.site_url().await {
        Ok(url) if !url.is_empty() => url,
        _ => crate::feeds::origin_from_headers(headers),
    }
}

use vyasa_db::content_models::PostStatus;
use vyasa_db::repo::PostFilter;

/// `GET /sitemap.xml` — published posts and pages.
pub async fn sitemap(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let pattern = crate::permalinks::pattern(&state).await;
    let base = base_url(&state, &headers).await;
    let filter = PostFilter {
        status: Some(PostStatus::Published),
        limit: 1000,
        ..PostFilter::default()
    };
    let Ok(rows) = state.posts.list(&filter).await else {
        return internal();
    };
    // Reusable block patterns have no public route, so listing them only
    // sends crawlers to 404s; nor do entries of a type no longer served
    // publicly.
    let public_custom = crate::policy::PublicTypes::load(&state.plugin_surface, &state.pool).await;
    let rows: Vec<_> = rows
        .into_iter()
        .filter(|p| public_custom.opens(p.post_type))
        .collect();
    let mut xml = String::from(r#"<?xml version="1.0" encoding="UTF-8"?>"#);
    xml.push_str("<urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">");

    // The home page. It was missing entirely: the sitemap listed every post
    // and page and left out the one URL the site is actually at, which is
    // the first thing a crawler looks for. Its `lastmod` is the newest
    // thing on it, since that is when the front page last changed.
    xml.push_str("<url><loc>");
    xml.push_str(&esc(&format!("{base}/")));
    xml.push_str("</loc>");
    if let Some(newest) = rows.iter().map(|p| p.updated_at).max() {
        xml.push_str("<lastmod>");
        xml.push_str(&newest.format("%Y-%m-%d").to_string());
        xml.push_str("</lastmod>");
    }
    xml.push_str("</url>");

    // Custom-type archives: /{type} lists a plugin type's entries, and a
    // crawler that only ever sees entry URLs never learns the shelf they
    // sit on. Only public types with archives — the same rule the router
    // applies.
    for decl in crate::entry_fields::served_types(&state.plugin_surface, &state.pool).await {
        if !decl.public || !decl.has_archive {
            continue;
        }
        // Only archives that hold something: an empty listing wastes a
        // crawl, and a URL without a lastmod would break the sitemap's own
        // rule that every entry says when it changed.
        let Some(newest) = rows
            .iter()
            .filter(|p| p.post_type.as_str() == decl.slug)
            .map(|p| p.updated_at)
            .max()
        else {
            continue;
        };
        xml.push_str("<url><loc>");
        xml.push_str(&esc(&format!("{base}/{}", decl.slug)));
        xml.push_str("</loc><lastmod>");
        xml.push_str(&newest.format("%Y-%m-%d").to_string());
        xml.push_str("</lastmod></url>");
    }

    for p in &rows {
        // A protected entry is `noindex` when fetched, so listing it only
        // sends a crawler to a page that then refuses to be indexed.
        if p.password_hash.is_some() {
            continue;
        }
        // The one permalink rule, not a second copy of it. The copy here
        // knew Page and "everything else", so a plugin's `book` entry —
        // published at /book/{slug} — was advertised at /post/{slug}, and
        // every custom-type URL a crawler was handed was a 404. The feeds
        // had the same bug and the same fix.
        let path = pattern.path_for(p);
        xml.push_str("<url><loc>");
        xml.push_str(&esc(&format!("{base}{path}")));
        // Without a `lastmod` a crawler has no way to tell an edited page
        // from an untouched one, so it either re-fetches everything or
        // re-fetches nothing. Date precision is deliberate: to the second
        // it changes on every republish and stops meaning anything.
        xml.push_str("</loc><lastmod>");
        xml.push_str(&p.updated_at.format("%Y-%m-%d").to_string());
        xml.push_str("</lastmod></url>");
    }
    xml.push_str("</urlset>");
    crate::public::cacheable::cacheable(
        &headers,
        "application/xml; charset=utf-8",
        crate::public::cacheable::CRAWLER_MAX_AGE,
        xml,
    )
}

/// `GET /robots.txt` — allow all, point at the sitemap.
pub async fn robots(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let base = base_url(&state, &headers).await;
    crate::public::cacheable::cacheable(
        &headers,
        "text/plain; charset=utf-8",
        crate::public::cacheable::CRAWLER_MAX_AGE,
        // The admin is disallowed not as a security measure — it requires
        // authentication regardless — but so crawlers do not spend their
        // budget on a login screen.
        format!("User-agent: *\nAllow: /\nDisallow: /admin\nSitemap: {base}/sitemap.xml\n"),
    )
}

/// `GET /llms.txt` — the site's index for AI answer engines.
///
/// The format (llmstxt.org) is deliberately plain markdown: a name, a
/// summary, then links. Every entry link points at its `.md` mirror, so
/// a model that follows one reads clean markdown instead of themed
/// HTML. Robots already welcomes crawlers; this tells the ones that
/// *answer* where to read.
pub async fn llms_txt(State(state): State<AppState>, headers: HeaderMap) -> Response {
    use std::fmt::Write as _;
    let pattern = crate::permalinks::pattern(&state).await;
    let base = base_url(&state, &headers).await;
    let identity = state.options_service.site_identity().await.ok();
    let filter = PostFilter {
        status: Some(PostStatus::Published),
        limit: 500,
        ..PostFilter::default()
    };
    let Ok(rows) = state.posts.list(&filter).await else {
        return internal();
    };
    let mut out = String::new();
    let title = identity
        .as_ref()
        .map(|i| i.title.clone())
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| base.clone());
    let _ = writeln!(out, "# {title}\n");
    if let Some(tagline) = identity
        .as_ref()
        .map(|i| i.tagline.clone())
        .filter(|t| !t.is_empty())
    {
        let _ = writeln!(out, "> {tagline}\n");
    }
    out.push_str(
        "Every link below serves clean markdown; drop the .md suffix for the themed page.\n",
    );
    // Grouped by type, pages first — a reader (human or model) scanning
    // this file wants the site's shape, not a flat pile.
    let mut sections: std::collections::BTreeMap<&'static str, Vec<String>> =
        std::collections::BTreeMap::new();
    let public_custom = crate::policy::PublicTypes::load(&state.plugin_surface, &state.pool).await;
    let mut defs = crate::entry_fields::Definitions::default();
    for p in rows.iter().filter(|p| public_custom.opens(p.post_type)) {
        // Protected entries are password-gated on the HTML route and 404
        // on the mirror; advertising them here would be a leak.
        if p.password_hash.is_some() {
            continue;
        }
        let path = pattern.path_for(p);
        let excerpt = p.excerpt.as_deref().unwrap_or("").trim().to_owned();
        let mut line = if excerpt.is_empty() {
            format!("- [{}]({base}{path}.md)", p.title)
        } else {
            format!("- [{}]({base}{path}.md): {excerpt}", p.title)
        };
        // Non-empty text fields, one line each, so a reader sees the
        // entry's facts without fetching it.
        for (label, value) in
            crate::entry_fields::text_values(defs.of(&state.pool, p.post_type).await, &p.meta)
        {
            let value = value.split_whitespace().collect::<Vec<_>>().join(" ");
            let _ = write!(line, "\n  - {label}: {value}");
        }
        sections.entry(p.post_type.as_str()).or_default().push(line);
    }
    for (kind, lines) in &sections {
        let heading = match *kind {
            "page" => "Pages".to_owned(),
            "post" => "Posts".to_owned(),
            other => {
                let mut c = other.chars();
                c.next().map_or_else(String::new, |f| {
                    f.to_uppercase().collect::<String>() + c.as_str() + "s"
                })
            }
        };
        let _ = write!(out, "\n## {heading}\n\n{}\n", lines.join("\n"));
    }
    crate::public::cacheable::cacheable(
        &headers,
        "text/plain; charset=utf-8",
        crate::public::cacheable::CRAWLER_MAX_AGE,
        out,
    )
}

/// `GET /indexnow.txt` — the ownership proof IndexNow pings point at
/// via `keyLocation`. 404 until the first publish mints a key.
pub async fn indexnow_key(State(state): State<AppState>, headers: HeaderMap) -> Response {
    match state.options.get(crate::indexnow::KEY_OPTION).await {
        Ok(v) => match v.as_str().filter(|k| !k.is_empty()) {
            Some(key) => crate::public::cacheable::cacheable(
                &headers,
                "text/plain; charset=utf-8",
                crate::public::cacheable::CRAWLER_MAX_AGE,
                key.to_owned(),
            ),
            None => StatusCode::NOT_FOUND.into_response(),
        },
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

fn esc(t: &str) -> String {
    t.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn internal() -> Response {
    (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
}
