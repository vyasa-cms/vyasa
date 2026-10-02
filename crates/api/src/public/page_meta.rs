//! What the public pages need beyond the post row itself: author names,
//! the site's date format, an excerpt when the author did not write one,
//! a post's taxonomy terms, and the `<head>` markup search engines and
//! social networks read.
//!
//! These are the joins the renderer cannot do — the themes crate has no
//! database — so they happen here and arrive as plain strings.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use vyasa_db::content_models::{PostRow, PostType};
use vyasa_themes::TermLink;

use crate::state::AppState;

/// Fallback when the site has not set `date_format`.
const DEFAULT_DATE_FORMAT: &str = "%b %e, %Y";
/// Longest generated excerpt, in characters.
const EXCERPT_CHARS: usize = 200;
/// Longest meta description, in characters. Search engines truncate
/// beyond roughly this.
const DESCRIPTION_CHARS: usize = 160;

/// Site-wide values every public render needs, read once per request.
#[derive(Debug, Clone)]
pub struct SiteContext {
    /// Absolute site URL without a trailing slash, when configured.
    pub base_url: String,
    /// Site title.
    pub title: String,
    /// Site tagline.
    pub tagline: String,
    /// `strftime` pattern for public dates.
    pub date_format: String,
    /// Site timezone for display.
    pub timezone: chrono_tz::Tz,
    /// Canonical post addresses.
    pub permalinks: vyasa_core::options::PermalinkPattern,
    /// Listing page size.
    pub per_page: i64,
    /// Media id of the favicon.
    pub favicon_media_id: Option<i64>,
    /// `<link>`/`<script>` tags for plugin-declared assets.
    ///
    /// Carried on the site context rather than added at each of the six
    /// places a `<head>` is assembled: an asset a plugin enqueued has to
    /// appear on every page, and "every page" is exactly what this type
    /// already describes.
    pub plugin_head: String,
}

impl SiteContext {
    /// Reads the options that shape every public page.
    ///
    /// `request_origin` is the fallback for a site that has not been told
    /// its own address — the same fallback the sitemap and the feeds have
    /// always used. Without it the head silently omitted `canonical` and
    /// `og:url` on every page, while those two files emitted absolute URLs
    /// perfectly well: three code paths, one source of truth between them.
    ///
    /// A canonical derived from a request header would be a cache-poisoning
    /// vector on its own, so [`super::routes::respond_cached`] keys cached
    /// pages by origin.
    ///
    /// Pass `""` from previews, which are neither cached nor indexed.
    pub async fn load(state: &AppState, request_origin: &str) -> Self {
        let identity = state.options_service.site_identity().await.ok();
        let string_opt = |key: &'static str| {
            let options = state.options.clone();
            async move {
                options
                    .get(key)
                    .await
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_owned))
                    .filter(|s| !s.trim().is_empty())
            }
        };
        let date_format = string_opt("date_format")
            .await
            .filter(|f| valid_date_format(f))
            .unwrap_or_else(|| DEFAULT_DATE_FORMAT.to_owned());
        let per_page = state
            .options
            .get("posts_per_page")
            .await
            .ok()
            .and_then(|v| v.as_i64())
            .filter(|n| (1..=100).contains(n))
            .unwrap_or(super::routes::PAGE_SIZE);
        Self {
            plugin_head: state.plugin_surface.head_html().await,
            base_url: string_opt("site_url").await.map_or_else(
                || request_origin.trim_end_matches('/').to_owned(),
                |u| u.trim_end_matches('/').to_owned(),
            ),
            title: identity
                .as_ref()
                .map(|i| i.title.clone())
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| "Vyasa".to_owned()),
            tagline: identity
                .as_ref()
                .map(|i| i.tagline.clone())
                .unwrap_or_default(),
            timezone: string_opt("timezone")
                .await
                .and_then(|s| s.parse().ok())
                .unwrap_or(chrono_tz::UTC),
            permalinks: crate::permalinks::pattern(state).await,
            date_format,
            per_page,
            favicon_media_id: identity.as_ref().and_then(|i| i.favicon_media_id),
        }
    }

    /// Formats a timestamp for display using the site's pattern.
    #[must_use]
    pub fn date(&self, at: DateTime<Utc>) -> String {
        at.with_timezone(&self.timezone)
            .format(&self.date_format)
            .to_string()
    }

    /// An absolute URL for `path`, when the site URL is configured.
    #[must_use]
    pub fn absolute(&self, path: &str) -> Option<String> {
        (!self.base_url.is_empty()).then(|| format!("{}{path}", self.base_url))
    }
}

/// Rejects a `date_format` containing anything but `strftime` specifiers
/// and ordinary punctuation, so a bad option cannot emit surprises.
pub(crate) fn valid_date_format(pattern: &str) -> bool {
    !pattern.is_empty()
        && pattern.len() <= 64
        && !pattern.contains('<')
        && !pattern.contains('>')
        && !pattern.contains('&')
}

/// Display names for a set of posts' authors, in one query.
pub async fn author_names(state: &AppState, rows: &[PostRow]) -> HashMap<i64, String> {
    let mut ids: Vec<i64> = rows.iter().map(|p| p.author_id).collect();
    ids.sort_unstable();
    ids.dedup();
    if ids.is_empty() {
        return HashMap::new();
    }
    let repo = vyasa_db::repo::UsersRepo::new(state.pool.clone());
    repo.display_names(&ids).await.unwrap_or_default()
}

/// The excerpt a listing should show: the author's own, or the opening
/// prose of the entry when they left it blank — which the editor's own
/// hint promises ("Left blank, the opening lines are used").
#[must_use]
pub fn excerpt_for(post: &PostRow) -> String {
    if let Some(text) = post
        .excerpt
        .as_ref()
        .map(|e| e.trim())
        .filter(|e| !e.is_empty())
    {
        return text.to_owned();
    }
    let doc: vyasa_core::BlockDocument = match serde_json::from_value(post.content.clone()) {
        Ok(doc) => doc,
        Err(_) => return String::new(),
    };
    let text = vyasa_themes::blocks_to_text(&doc.blocks);
    truncate_words(&text, EXCERPT_CHARS)
}

/// Cuts at a word boundary and adds an ellipsis, or returns the whole
/// string when it already fits.
#[must_use]
pub fn truncate_words(text: &str, max_chars: usize) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= max_chars {
        return trimmed.to_owned();
    }
    let mut out = String::with_capacity(max_chars + 1);
    for word in trimmed.split_whitespace() {
        if out.chars().count() + word.chars().count() + 1 > max_chars {
            break;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    if out.is_empty() {
        out = trimmed.chars().take(max_chars).collect();
    }
    out.push('…');
    out
}

/// The categories and tags of one post, as archive links.
pub async fn terms_for(state: &AppState, post_id: i64) -> Vec<TermLink> {
    let repo = vyasa_db::repo::TermsRepo::new(state.pool.clone());
    let Ok(ids) = repo.list_for_post(post_id).await else {
        return Vec::new();
    };
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        if let Ok(term) = repo.get(id).await {
            // Every taxonomy, built in or plugin-registered, archives at
            // `/{taxonomy}/{slug}`.
            let taxonomy = term.taxonomy.as_str();
            out.push(TermLink {
                name: term.name,
                url: format!("/{taxonomy}/{}", term.slug),
                taxonomy: taxonomy.to_owned(),
            });
        }
    }
    out.sort_by(|a, b| a.taxonomy.cmp(&b.taxonomy).then(a.name.cmp(&b.name)));
    out
}

/// Everything that goes between `<head>` tags beyond the basics the base
/// template already emits.
#[derive(Debug, Clone, Default)]
pub struct Head {
    /// Canonical path of this page (site-absolute, e.g. `/post/x`).
    pub path: String,
    /// Meta description.
    pub description: String,
    /// `og:type` — `article` for entries, `website` elsewhere.
    pub kind: &'static str,
    /// Absolute image URL for social cards.
    pub image: Option<String>,
    /// Publication timestamp for `article:published_time`.
    pub published: Option<DateTime<Utc>>,
    /// Last-modified timestamp.
    pub modified: Option<DateTime<Utc>>,
    /// Author display name.
    pub author: Option<String>,
    /// Title used for the social cards; defaults to the page title.
    pub title: String,
    /// Whether search engines should index this page.
    pub indexable: bool,
    /// Whether search engines may follow links on this page.
    pub follow: bool,
    /// An absolute canonical the author set, for syndicated content.
    pub canonical: Option<String>,
    /// Social overrides: title, description, image for feeds.
    pub social_title: Option<String>,
    pub social_description: Option<String>,
    /// A second JSON-LD document (FAQ, HowTo, Product) beside Article.
    pub structured: Option<serde_json::Value>,
    /// Translations of this page: (language tag, absolute or site path).
    pub alternates: Vec<(String, String)>,
}

impl Head {
    /// Attaches the page's translations, for hreflang links.
    #[must_use]
    pub fn with_alternates(mut self, alternates: Vec<(String, String)>) -> Self {
        self.alternates = alternates;
        self
    }

    /// A page that is not an entry (home, archive, search).
    #[must_use]
    pub fn page(path: &str, title: &str, description: &str) -> Self {
        Self {
            path: path.to_owned(),
            description: description.to_owned(),
            kind: "website",
            title: title.to_owned(),
            indexable: true,
            follow: true,
            ..Self::default()
        }
    }

    /// The head for one entry, honouring the `seo_title` and
    /// `seo_description` the editor's Search preview panel writes (and
    /// that AI autofill fills in) before falling back to the entry itself.
    #[must_use]
    pub fn entry(post: &PostRow, path: &str, author: Option<String>) -> Self {
        let meta = |key: &str| {
            post.meta
                .get(key)
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(str::to_owned)
        };
        let description = meta("seo_description")
            .unwrap_or_else(|| truncate_words(&excerpt_for(post), DESCRIPTION_CHARS));
        // The picture the author already placed, when they have not set a
        // separate social image. Asking someone to nominate an "SEO image"
        // for the photo at the top of their own post is a question with an
        // obvious answer.
        let flag =
            |key: &str| post.meta.get(key).and_then(serde_json::Value::as_bool) == Some(true);
        let image = meta("og_image")
            .or_else(|| meta("seo_image"))
            .or_else(|| meta("featured_media_url"))
            .or_else(|| {
                serde_json::from_value::<vyasa_core::BlockDocument>(post.content.clone())
                    .ok()
                    .and_then(|doc| vyasa_themes::first_image(&doc.blocks))
            });
        Self {
            alternates: Vec::new(),
            path: path.to_owned(),
            description,
            kind: "article",
            image,
            published: post.published_at,
            modified: Some(post.updated_at),
            author,
            title: meta("seo_title").unwrap_or_else(|| post.title.clone()),
            indexable: post.password_hash.is_none() && !flag("seo_noindex"),
            follow: !flag("seo_nofollow"),
            canonical: meta("seo_canonical")
                .filter(|c| c.starts_with("https://") || c.starts_with("http://")),
            social_title: meta("og_title"),
            social_description: meta("og_description"),
            structured: crate::seo::structured_data(post, path),
        }
    }

    /// Renders the markup. Absolute URLs are emitted only when the site
    /// knows its own address; a relative canonical is worse than none.
    #[must_use]
    /// The same head, with its title and description passed through the
    /// named filter points.
    ///
    /// Separate from `render` because filtering crosses a sandbox boundary
    /// and `render` is a pure string builder used in tests. Callers chain
    /// the two: `head.filtered(state).await.render(&site)`.
    pub async fn filtered(mut self, state: &AppState) -> Self {
        self.title =
            crate::plugin_hooks::filter(state, crate::plugin_hooks::filters::SEO_TITLE, self.title)
                .await;
        self.description = crate::plugin_hooks::filter(
            state,
            crate::plugin_hooks::filters::SEO_DESCRIPTION,
            self.description,
        )
        .await;
        self
    }

    pub fn render(&self, site: &SiteContext) -> String {
        use std::fmt::Write as _;
        let esc = escape;
        let mut out = String::new();
        if !site.plugin_head.is_empty() {
            out.push('\n');
            out.push_str(&site.plugin_head);
        }
        if !self.description.is_empty() {
            let _ = write!(
                out,
                "\n<meta name=\"description\" content=\"{}\">",
                esc(&self.description)
            );
        }
        for (lang, path) in &self.alternates {
            let href = site.absolute(path).unwrap_or_else(|| path.clone());
            let _ = write!(
                out,
                "\n<link rel=\"alternate\" hreflang=\"{}\" href=\"{}\">",
                esc(lang),
                esc(&href)
            );
        }
        if let Some(url) = self.canonical.clone().or_else(|| site.absolute(&self.path)) {
            let _ = write!(out, "\n<link rel=\"canonical\" href=\"{}\">", esc(&url));
            let _ = write!(
                out,
                "\n<meta property=\"og:url\" content=\"{}\">",
                esc(&url)
            );
            if self.kind == "article" {
                // The markdown mirror, for AI readers that check the head.
                let _ = write!(
                    out,
                    "\n<link rel=\"alternate\" type=\"text/markdown\" href=\"{}.md\">",
                    esc(&url)
                );
            }
        }
        let title = if self.title.is_empty() {
            site.title.clone()
        } else {
            self.title.clone()
        };
        out.push_str(&self.social_tags(site, &title));
        if let Some(image) = self.image.as_ref().and_then(|i| absolute_media(site, i)) {
            let _ = write!(
                out,
                "\n<meta property=\"og:image\" content=\"{}\">\
                 \n<meta name=\"twitter:card\" content=\"summary_large_image\">",
                esc(&image)
            );
        } else {
            out.push_str("\n<meta name=\"twitter:card\" content=\"summary\">");
        }
        if let Some(published) = self.published {
            let _ = write!(
                out,
                "\n<meta property=\"article:published_time\" content=\"{}\">",
                published.to_rfc3339()
            );
        }
        if let Some(modified) = self.modified {
            let _ = write!(
                out,
                "\n<meta property=\"article:modified_time\" content=\"{}\">",
                modified.to_rfc3339()
            );
        }
        if let Some(author) = &self.author {
            let _ = write!(out, "\n<meta name=\"author\" content=\"{}\">", esc(author));
        }
        out.push_str(self.robots_tag());
        // Feeds and the icon are site-wide, so every page advertises them.
        let _ = write!(
            out,
            "\n<link rel=\"alternate\" type=\"application/rss+xml\" title=\"{} · RSS\" href=\"/feed.xml\">\
             \n<link rel=\"alternate\" type=\"application/atom+xml\" title=\"{} · Atom\" href=\"/atom.xml\">",
            esc(&site.title),
            esc(&site.title)
        );
        if site.favicon_media_id.is_some() {
            out.push_str("\n<link rel=\"icon\" href=\"/favicon.ico\">");
        } else {
            out.push_str("\n<link rel=\"icon\" type=\"image/svg+xml\" href=\"/brand/mark.svg\">");
        }
        out.push_str(&self.json_ld(site));
        out
    }

    /// Structured data, as one `application/ld+json` block.
    ///
    /// This is what search engines read for rich results — an article's
    /// byline and dates, and a site's search box. Open Graph tags are for
    /// social previews and are not a substitute; nothing here emitted any
    /// structured data at all.
    ///
    /// Absolute URLs only: a `@id` or `url` that is a path is not usable
    /// by a consumer, so a site that has not been told its address emits
    /// nothing rather than something wrong.
    /// The robots meta, or nothing when the page is plainly indexable.
    fn robots_tag(&self) -> &'static str {
        match (self.indexable, self.follow) {
            (false, false) => "\n<meta name=\"robots\" content=\"noindex, nofollow\">",
            (false, true) => "\n<meta name=\"robots\" content=\"noindex\">",
            (true, false) => "\n<meta name=\"robots\" content=\"nofollow\">",
            (true, true) => "",
        }
    }

    /// The social card tags: title and description for feeds, which fall
    /// back to the search ones.
    fn social_tags(&self, site: &SiteContext, title: &str) -> String {
        use std::fmt::Write as _;
        let esc = escape;
        let mut out = String::new();
        let social_title = self
            .social_title
            .clone()
            .unwrap_or_else(|| title.to_owned());
        let social_description = self
            .social_description
            .clone()
            .unwrap_or_else(|| self.description.clone());
        let _ = write!(
            out,
            "\n<meta property=\"og:type\" content=\"{}\">\
             \n<meta property=\"og:title\" content=\"{}\">\
             \n<meta name=\"twitter:title\" content=\"{}\">\
             \n<meta property=\"og:site_name\" content=\"{}\">",
            self.kind,
            esc(&social_title),
            esc(&social_title),
            esc(&site.title)
        );
        if !social_description.is_empty() {
            let _ = write!(
                out,
                "\n<meta property=\"og:description\" content=\"{}\">\
                 \n<meta name=\"twitter:description\" content=\"{}\">",
                esc(&social_description),
                esc(&social_description)
            );
        }
        out
    }

    fn json_ld(&self, site: &SiteContext) -> String {
        let main = self.json_ld_main(site);
        match &self.structured {
            Some(doc) => format!(
                "{main}\n<script type=\"application/ld+json\">{}</script>",
                serde_json::to_string(doc)
                    .unwrap_or_default()
                    .replace("</", "<\\/")
            ),
            None => main,
        }
    }

    fn json_ld_main(&self, site: &SiteContext) -> String {
        let Some(url) = site.absolute(&self.path) else {
            return String::new();
        };
        let doc = if self.kind == "article" {
            let mut article = serde_json::json!({
                "@context": "https://schema.org",
                "@type": "Article",
                "headline": self.title,
                "mainEntityOfPage": {"@type": "WebPage", "@id": url},
                "url": url,
            });
            let map = article.as_object_mut().unwrap_or_else(|| unreachable!());
            if !self.description.is_empty() {
                map.insert("description".into(), self.description.clone().into());
            }
            if let Some(published) = self.published {
                map.insert("datePublished".into(), published.to_rfc3339().into());
            }
            if let Some(modified) = self.modified {
                map.insert("dateModified".into(), modified.to_rfc3339().into());
            }
            if let Some(author) = &self.author {
                map.insert(
                    "author".into(),
                    serde_json::json!({"@type": "Person", "name": author}),
                );
            }
            if let Some(image) = self.image.as_ref().and_then(|i| absolute_media(site, i)) {
                map.insert("image".into(), image.into());
            }
            if !site.title.is_empty() {
                map.insert(
                    "publisher".into(),
                    serde_json::json!({"@type": "Organization", "name": site.title}),
                );
            }
            article
        } else if self.path == "/" {
            // The home page describes the site itself, and advertises the
            // search endpoint so a result can carry a sitelinks searchbox.
            serde_json::json!({
                "@context": "https://schema.org",
                "@type": "WebSite",
                "name": site.title,
                "url": site.absolute("/").unwrap_or_else(|| url.clone()),
                "potentialAction": {
                    "@type": "SearchAction",
                    "target": {
                        "@type": "EntryPoint",
                        "urlTemplate": format!("{}/search?s={{search_term_string}}", site.base_url),
                    },
                    "query-input": "required name=search_term_string",
                },
            })
        } else {
            return String::new();
        };
        // `</script>` inside a JSON string would close this element early;
        // serde escapes quotes but not that sequence.
        let json = doc.to_string().replace("</", "<\\/");
        format!("\n<script type=\"application/ld+json\">{json}</script>")
    }
}

/// Turns a stored image reference into an absolute URL when possible.
fn absolute_media(site: &SiteContext, reference: &str) -> Option<String> {
    if reference.starts_with("http://") || reference.starts_with("https://") {
        return Some(reference.to_owned());
    }
    if reference.starts_with('/') {
        return site.absolute(reference);
    }
    reference
        .parse::<i64>()
        .ok()
        .and_then(|id| site.absolute(&format!("/api/v1/media/{id}/raw")))
}

/// Escapes a value for an HTML attribute.
fn escape(raw: &str) -> String {
    raw.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Whether a post type is publicly addressable. Reusable block patterns
/// are not: they have no route, so listing them in a feed or a sitemap
/// only produces 404s.
#[must_use]
pub const fn is_public_type(t: PostType) -> bool {
    // Custom types are public unless their declaration said otherwise;
    // the router checks that flag before it gets here, and a `block` is
    // never a page.
    matches!(t, PostType::Post | PostType::Page) || t.is_custom()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncation_stops_at_a_word_and_marks_the_cut() {
        assert_eq!(truncate_words("short text", 50), "short text");
        let long = "the quick brown fox jumps over the lazy dog";
        let cut = truncate_words(long, 20);
        assert!(cut.ends_with('…'), "{cut}");
        assert!(cut.chars().count() <= 21, "{cut}");
        assert!(!cut.contains("jump"), "cut mid-word: {cut}");
        // A single word longer than the budget is cut hard rather than lost.
        assert!(truncate_words("supercalifragilistic", 5).starts_with("super"));
    }

    #[test]
    fn attribute_escaping_closes_no_tags() {
        let nasty = r#"a "quoted" <script>&"#;
        let escaped = escape(nasty);
        assert!(!escaped.contains('<'));
        assert!(!escaped.contains('"'));
        assert_eq!(escaped, "a &quot;quoted&quot; &lt;script&gt;&amp;");
    }

    #[test]
    fn date_formats_that_could_inject_markup_are_refused() {
        assert!(valid_date_format("%Y-%m-%d"));
        assert!(valid_date_format("%b %e, %Y"));
        assert!(!valid_date_format(""));
        assert!(!valid_date_format("<script>"));
        assert!(!valid_date_format(&"%Y".repeat(40)));
    }

    #[test]
    fn only_routable_types_are_public() {
        assert!(is_public_type(PostType::Post));
        assert!(is_public_type(PostType::Page));
        // A reusable pattern is content for other posts, not a page: it
        // must stay out of feeds and the sitemap.
        assert!(!is_public_type(PostType::Block));
        // A plugin's type has its own URLs, so it belongs in both. The
        // router checks the declaration's `public` flag before this.
        assert!(is_public_type(PostType::Custom("book")));
    }
}
