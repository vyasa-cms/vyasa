//! SEO plumbing the editors lean on: redirects written when a live
//! entry's address changes, outbound link checks, and the signals the
//! sidebar shows (inbound links, keyphrase rivals).

use serde::Serialize;
use sqlx::PgPool;
use vyasa_common::AppError;
#[cfg(test)]
use vyasa_db::content_models::PostType;
use vyasa_db::content_models::{PostRow, PostStatus};

/// The public path of an entry.
#[cfg(test)]
#[must_use]
pub fn public_path(post_type: &PostType, slug: &str) -> String {
    match post_type {
        PostType::Page => format!("/{slug}"),
        PostType::Custom(t) => format!("/{t}/{slug}"),
        _ => format!("/post/{slug}"),
    }
}

/// Where a path redirects to, if anywhere.
///
/// # Errors
/// [`AppError::Db`] on failure.
pub async fn redirect_for(pool: &PgPool, path: &str) -> Result<Option<String>, AppError> {
    sqlx::query_scalar::<_, String>("SELECT to_path FROM redirects WHERE from_path = $1")
        .bind(path)
        .fetch_optional(pool)
        .await
        .map_err(|e| AppError::db(format!("redirect lookup: {e}")))
}

/// Records `from → to`, and removes any redirect that pointed at `from`
/// the other way, so a rename and a rename back never loop. Redirects
/// that pointed at `from` now point at `to` (chains are collapsed).
///
/// # Errors
/// [`AppError::Db`] on failure.
pub async fn add_redirect(pool: &PgPool, from: &str, to: &str) -> Result<(), AppError> {
    if from == to || from == "/" {
        return Ok(());
    }
    let mut tx = pool
        .begin()
        .await
        .map_err(|e| AppError::db(format!("redirect: {e}")))?;
    sqlx::query("DELETE FROM redirects WHERE from_path = $1")
        .bind(to)
        .execute(&mut *tx)
        .await
        .map_err(|e| AppError::db(format!("redirect: {e}")))?;
    sqlx::query("UPDATE redirects SET to_path = $2 WHERE to_path = $1")
        .bind(from)
        .bind(to)
        .execute(&mut *tx)
        .await
        .map_err(|e| AppError::db(format!("redirect: {e}")))?;
    sqlx::query(
        "INSERT INTO redirects (from_path, to_path) VALUES ($1, $2)
         ON CONFLICT (from_path) DO UPDATE SET to_path = EXCLUDED.to_path",
    )
    .bind(from)
    .bind(to)
    .execute(&mut *tx)
    .await
    .map_err(|e| AppError::db(format!("redirect: {e}")))?;
    tx.commit()
        .await
        .map_err(|e| AppError::db(format!("redirect: {e}")))
}

/// One redirect, as listed.
#[derive(Serialize, utoipa::ToSchema, sqlx::FromRow)]
pub struct Redirect {
    /// Old path.
    pub from_path: String,
    /// Where it goes.
    pub to_path: String,
    /// When it was written.
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Every redirect, newest first.
///
/// # Errors
/// [`AppError::Db`] on failure.
pub async fn list_redirects(pool: &PgPool) -> Result<Vec<Redirect>, AppError> {
    sqlx::query_as::<_, Redirect>(
        "SELECT from_path, to_path, created_at FROM redirects ORDER BY created_at DESC LIMIT 500",
    )
    .fetch_all(pool)
    .await
    .map_err(|e| AppError::db(format!("redirects: {e}")))
}

/// Removes one redirect. `Ok(false)` when there was none.
///
/// # Errors
/// [`AppError::Db`] on failure.
pub async fn delete_redirect(pool: &PgPool, from: &str) -> Result<bool, AppError> {
    let done = sqlx::query("DELETE FROM redirects WHERE from_path = $1")
        .bind(from)
        .execute(pool)
        .await
        .map_err(|e| AppError::db(format!("redirect: {e}")))?;
    Ok(done.rows_affected() > 0)
}

/// Every `href` in an entry's inline fields.
#[must_use]
pub fn hrefs_of(post: &PostRow) -> Vec<String> {
    fn walk(blocks: &[vyasa_core::block::Block], out: &mut Vec<String>) {
        for b in blocks {
            for field in ["text", "summary", "caption", "title"] {
                if let Some(t) = b.attrs.get(field).and_then(serde_json::Value::as_str) {
                    let mut rest = t;
                    while let Some(i) = rest.find("href=\"") {
                        rest = &rest[i + 6..];
                        if let Some(end) = rest.find('"') {
                            out.push(rest[..end].replace("&amp;", "&"));
                            rest = &rest[end..];
                        }
                    }
                }
            }
            if b.kind == vyasa_core::block::BlockKind::Button {
                if let Some(h) = b.attrs.get("href").and_then(serde_json::Value::as_str) {
                    out.push(h.to_owned());
                }
            }
            walk(&b.children, out);
        }
    }
    let mut out = Vec::new();
    if let Ok(doc) = serde_json::from_value::<vyasa_core::BlockDocument>(post.content.clone()) {
        walk(&doc.blocks, &mut out);
    }
    out.sort();
    out.dedup();
    out
}

/// One outbound link that did not answer well.
#[derive(Serialize, serde::Deserialize, Clone, utoipa::ToSchema)]
pub struct BrokenLink {
    /// The address as written.
    pub url: String,
    /// HTTP status, or 0 when the host did not answer.
    pub status: u16,
}

/// The stored result of the last check.
#[derive(Serialize, utoipa::ToSchema)]
pub struct LinkCheck {
    /// When the links were last fetched.
    pub checked_at: chrono::DateTime<chrono::Utc>,
    /// Links that did not answer 2xx/3xx.
    pub broken: Vec<BrokenLink>,
    /// How many outbound links were checked.
    pub checked: usize,
}

const MAX_LINKS_PER_POST: usize = 60;

/// Redirect hops the link checker follows before calling a link broken.
const MAX_LINK_REDIRECTS: usize = 5;

/// The link checker's client: guarded, because the links come from post
/// content any author writes, and a "link" to `http://169.254.169.254/` or
/// to a name resolving to the server's own network must not become a
/// request from inside it — on the first hop or any redirect after.
fn link_check_client() -> &'static reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(|| {
        crate::net_guard::guarded_builder(MAX_LINK_REDIRECTS)
            .timeout(std::time::Duration::from_secs(8))
            .user_agent("Mozilla/5.0 (compatible; Vyasa link check)")
            .build()
            .unwrap_or_default()
    })
}

/// Status of one outbound link, 0 when it could not (or may not) be
/// fetched.
async fn link_status(client: &reqwest::Client, raw: &str) -> u16 {
    let Ok(url) = url::Url::parse(raw) else {
        return 0;
    };
    // IP-literal hosts never reach the guarded resolver; check them here.
    if !crate::net_guard::url_allowed(&url) {
        return 0;
    }
    match client.head(url.clone()).send().await {
        Ok(r) if r.status().as_u16() == 405 || r.status().as_u16() == 403 => {
            // Some hosts refuse HEAD; ask properly before judging.
            client
                .get(url)
                .send()
                .await
                .map_or(0, |r| r.status().as_u16())
        }
        Ok(r) => r.status().as_u16(),
        Err(_) => 0,
    }
}

/// Fetches every external link in an entry and records what is broken.
///
/// Internal paths are not fetched: the site knows its own routes, and
/// checking them from inside would only measure the server against
/// itself.
///
/// # Errors
/// [`AppError::Db`] on failure to store the result.
pub async fn check_links(pool: &PgPool, post: &PostRow) -> Result<LinkCheck, AppError> {
    let client = link_check_client();
    let external: Vec<String> = hrefs_of(post)
        .into_iter()
        .filter(|h| h.starts_with("https://") || h.starts_with("http://"))
        .take(MAX_LINKS_PER_POST)
        .collect();
    let mut broken = Vec::new();
    for url in &external {
        let status = link_status(client, url).await;
        if !(200..400).contains(&status) {
            broken.push(BrokenLink {
                url: url.clone(),
                status,
            });
        }
    }
    let now = chrono::Utc::now();
    sqlx::query(
        "INSERT INTO link_checks (post_id, checked_at, broken) VALUES ($1, $2, $3)
         ON CONFLICT (post_id) DO UPDATE SET checked_at = EXCLUDED.checked_at, broken = EXCLUDED.broken",
    )
    .bind(post.id)
    .bind(now)
    .bind(serde_json::to_value(&broken).unwrap_or_default())
    .execute(pool)
    .await
    .map_err(|e| AppError::db(format!("link check: {e}")))?;
    Ok(LinkCheck {
        checked_at: now,
        broken,
        checked: external.len(),
    })
}

/// The stored result for an entry, if it has been checked.
///
/// # Errors
/// [`AppError::Db`] on failure.
pub async fn last_check(pool: &PgPool, post_id: i64) -> Result<Option<LinkCheck>, AppError> {
    let row = sqlx::query_as::<_, (chrono::DateTime<chrono::Utc>, serde_json::Value)>(
        "SELECT checked_at, broken FROM link_checks WHERE post_id = $1",
    )
    .bind(post_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| AppError::db(format!("link check: {e}")))?;
    Ok(row.map(|(checked_at, broken)| {
        let broken: Vec<BrokenLink> = serde_json::from_value(broken).unwrap_or_default();
        LinkCheck {
            checked_at,
            checked: 0,
            broken,
        }
    }))
}

/// How many published entries carry a broken outbound link.
///
/// # Errors
/// [`AppError::Db`] on failure.
pub async fn posts_with_broken_links(pool: &PgPool) -> Result<i64, AppError> {
    sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM link_checks c JOIN posts p ON p.id = c.post_id
         WHERE p.status = 'published' AND jsonb_array_length(c.broken) > 0",
    )
    .fetch_one(pool)
    .await
    .map_err(|e| AppError::db(format!("link checks: {e}")))
}

/// The nightly sweep: the published entries checked longest ago (or
/// never), a batch at a time, so a large site is covered over a few
/// nights without hammering anyone's server in one go.
pub async fn sweep_once(pool: &PgPool, batch: i64) -> usize {
    let ids: Vec<i64> = sqlx::query_scalar::<_, i64>(
        "SELECT p.id FROM posts p LEFT JOIN link_checks c ON c.post_id = p.id
         WHERE p.status = 'published' AND (c.checked_at IS NULL OR c.checked_at < now() - interval '1 day')
         ORDER BY c.checked_at ASC NULLS FIRST LIMIT $1",
    )
    .bind(batch)
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    let repo = vyasa_db::repo::PostsRepo::new(pool.clone());
    let mut done = 0;
    for id in ids {
        if let Ok(post) = repo.get(id).await {
            if check_links(pool, &post).await.is_ok() {
                done += 1;
            }
        }
    }
    done
}

/// Spawns the sweep: once an hour it checks a batch of entries whose
/// last check is a day old or older.
///
/// `_client` is ignored: link checks always use the SSRF-guarded client
/// (see `link_check_client`), whatever the caller passes. The parameter
/// stays only so the composition root need not change in step.
pub fn spawn_sweep(pool: PgPool) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(60 * 60));
        loop {
            interval.tick().await;
            let n = sweep_once(&pool, 25).await;
            if n > 0 {
                tracing::info!("link sweep checked {n} entries");
            }
        }
    })
}

/// What the sidebar shows about an entry's place on the site.
#[derive(Serialize, utoipa::ToSchema)]
pub struct SeoSignals {
    /// Published entries that link to this one.
    pub inbound_links: i64,
    /// Published entries that target the same keyphrase.
    pub keyphrase_rivals: Vec<Rival>,
    /// Old addresses that redirect here.
    pub redirects_here: Vec<String>,
}

/// Another entry competing for the same keyphrase.
#[derive(Serialize, utoipa::ToSchema)]
pub struct Rival {
    /// Entry id, as a string.
    pub id: String,
    /// Its title.
    pub title: String,
}

/// # Errors
/// [`AppError::Db`] on failure.
pub async fn signals(
    pool: &PgPool,
    post: &PostRow,
    site_url: Option<&str>,
) -> Result<SeoSignals, AppError> {
    let pattern =
        vyasa_core::options::OptionsService::new(vyasa_db::repo::OptionsRepo::new(pool.clone()))
            .permalink_pattern()
            .await?;
    let path = pattern.path_for(post);
    let like = |needle: String| format!("%{}%", needle.replace('\\', "\\\\").replace('%', "\\%"));
    // A link written as an absolute URL to this site counts too.
    let absolute = site_url
        .map(|u| u.trim_end_matches('/'))
        .filter(|u| !u.is_empty())
        .map_or_else(
            || like(format!("href=\"{path}\"")),
            |u| like(format!("href=\"{u}{path}\"")),
        );
    let inbound_links = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM posts WHERE status = 'published' AND id <> $1
         AND (content::text LIKE $2 OR content::text LIKE $3)",
    )
    .bind(post.id)
    .bind(like(format!("href=\"{path}\"")))
    .bind(absolute)
    .fetch_one(pool)
    .await
    .map_err(|e| AppError::db(format!("inbound links: {e}")))?;
    let keyphrase = post
        .meta
        .get("seo_keyphrase")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|k| !k.is_empty());
    let keyphrase_rivals = match keyphrase {
        None => Vec::new(),
        Some(k) => sqlx::query_as::<_, (i64, String)>(
            "SELECT id, title FROM posts WHERE status = 'published' AND id <> $1
             AND lower(meta->>'seo_keyphrase') = lower($2) LIMIT 10",
        )
        .bind(post.id)
        .bind(k)
        .fetch_all(pool)
        .await
        .map_err(|e| AppError::db(format!("keyphrase rivals: {e}")))?
        .into_iter()
        .map(|(id, title)| Rival {
            id: id.to_string(),
            title,
        })
        .collect(),
    };
    let redirects_here =
        sqlx::query_scalar::<_, String>("SELECT from_path FROM redirects WHERE to_path = $1")
            .bind(&path)
            .fetch_all(pool)
            .await
            .map_err(|e| AppError::db(format!("redirects: {e}")))?;
    Ok(SeoSignals {
        inbound_links,
        keyphrase_rivals,
        redirects_here,
    })
}

/// `Article` plus a richer type when the author chose one, built from
/// the blocks: FAQ from details blocks, HowTo from the first ordered
/// list, Product from the price fields.
#[must_use]
pub fn structured_data(post: &PostRow, url: &str) -> Option<serde_json::Value> {
    let kind = post
        .meta
        .get("schema_type")
        .and_then(serde_json::Value::as_str)?;
    let doc = serde_json::from_value::<vyasa_core::BlockDocument>(post.content.clone()).ok()?;
    let text = |v: Option<&serde_json::Value>| -> String {
        strip_tags(v.and_then(serde_json::Value::as_str).unwrap_or(""))
    };
    match kind {
        "faq" => {
            let mut qa = Vec::new();
            collect_faq(&doc.blocks, &mut qa);
            if qa.is_empty() {
                return None;
            }
            Some(serde_json::json!({
                "@context": "https://schema.org",
                "@type": "FAQPage",
                "mainEntity": qa.iter().map(|(q, a)| serde_json::json!({
                    "@type": "Question", "name": q,
                    "acceptedAnswer": {"@type": "Answer", "text": a}
                })).collect::<Vec<_>>()
            }))
        }
        "howto" => {
            let list = doc.blocks.iter().find(|b| {
                b.kind == vyasa_core::block::BlockKind::List
                    && b.attrs.get("ordered").and_then(serde_json::Value::as_bool) == Some(true)
            })?;
            let steps: Vec<serde_json::Value> = list
                .children
                .iter()
                .map(|c| text(c.attrs.get("text")))
                .filter(|t| !t.is_empty())
                .enumerate()
                .map(|(i, t)| serde_json::json!({"@type": "HowToStep", "position": i + 1, "text": t}))
                .collect();
            if steps.is_empty() {
                return None;
            }
            Some(serde_json::json!({
                "@context": "https://schema.org",
                "@type": "HowTo",
                "name": post.title,
                "step": steps,
            }))
        }
        "product" => {
            let price = post
                .meta
                .get("product_price")
                .and_then(serde_json::Value::as_str)?;
            let currency = post
                .meta
                .get("product_currency")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("USD");
            Some(serde_json::json!({
                "@context": "https://schema.org",
                "@type": "Product",
                "name": post.title,
                "url": url,
                "offers": {
                    "@type": "Offer", "price": price, "priceCurrency": currency,
                    "availability": "https://schema.org/InStock", "url": url
                }
            }))
        }
        _ => None,
    }
}

/// Question/answer pairs from Details blocks, for FAQ structured data.
fn collect_faq(blocks: &[vyasa_core::block::Block], out: &mut Vec<(String, String)>) {
    let text = |v: Option<&serde_json::Value>| {
        strip_tags(v.and_then(serde_json::Value::as_str).unwrap_or(""))
    };
    for b in blocks {
        if b.kind == vyasa_core::block::BlockKind::Details {
            let q = text(b.attrs.get("summary"));
            let a = b
                .children
                .iter()
                .map(|c| text(c.attrs.get("text")))
                .filter(|t| !t.is_empty())
                .collect::<Vec<_>>()
                .join(" ");
            if !q.is_empty() && !a.is_empty() {
                out.push((q, a));
            }
        }
        collect_faq(&b.children, out);
    }
}

/// Inline HTML as plain words, for structured data.
#[must_use]
pub fn strip_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#x27;", "'")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Whether a stored status belongs to a live entry.
#[must_use]
pub fn is_live(status: PostStatus) -> bool {
    matches!(
        status,
        PostStatus::Published | PostStatus::Scheduled | PostStatus::Private
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn post(content: serde_json::Value, meta: serde_json::Value) -> PostRow {
        PostRow {
            id: 1,
            post_type: PostType::Post,
            status: PostStatus::Published,
            slug: "hello".into(),
            title: "Hello".into(),
            content,
            layout: None,
            excerpt: None,
            author_id: 1,
            parent_id: None,
            meta,
            password_hash: None,
            published_at: None,
            scheduled_for: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            sticky: false,
            lang: String::new(),
            translation_group: None,
        }
    }

    #[test]
    fn public_paths_follow_the_type() {
        assert_eq!(public_path(&PostType::Post, "a"), "/post/a");
        assert_eq!(public_path(&PostType::Page, "a"), "/a");
        assert_eq!(public_path(&PostType::Custom("book"), "a"), "/book/a");
    }

    #[test]
    fn hrefs_come_from_every_inline_field_and_buttons() {
        let p = post(
            serde_json::json!({"schema_version": 1, "blocks": [
                {"kind": "paragraph", "attrs": {"text": "See <a href=\"https://a.example/x?a=1&amp;b=2\">this</a> and <a href=\"/post/other\">that</a>"}},
                {"kind": "buttons", "children": [{"kind": "button", "attrs": {"label": "Go", "href": "https://b.example/"}}]},
                {"kind": "details", "attrs": {"summary": "<a href=\"https://a.example/x?a=1&amp;b=2\">dup</a>"}, "children": []}
            ]}),
            serde_json::json!({}),
        );
        assert_eq!(
            hrefs_of(&p),
            vec![
                "/post/other",
                "https://a.example/x?a=1&b=2",
                "https://b.example/"
            ]
        );
    }

    #[test]
    fn structured_data_is_built_from_the_blocks() {
        let faq = post(
            serde_json::json!({"schema_version": 1, "blocks": [
                {"kind": "details", "attrs": {"summary": "Why?"}, "children": [{"kind": "paragraph", "attrs": {"text": "Because <b>so</b>."}}]}
            ]}),
            serde_json::json!({"schema_type": "faq"}),
        );
        let doc = structured_data(&faq, "https://s.example/post/hello").expect("faq");
        assert_eq!(doc["@type"], "FAQPage");
        assert_eq!(
            doc["mainEntity"][0]["acceptedAnswer"]["text"],
            "Because so."
        );

        let howto = post(
            serde_json::json!({"schema_version": 1, "blocks": [
                {"kind": "list", "attrs": {"ordered": true}, "children": [
                    {"kind": "paragraph", "attrs": {"text": "First"}}, {"kind": "paragraph", "attrs": {"text": "Second"}}]}
            ]}),
            serde_json::json!({"schema_type": "howto"}),
        );
        let doc = structured_data(&howto, "u").expect("howto");
        assert_eq!(doc["step"][1]["position"], 2);

        let product = post(
            serde_json::json!({"schema_version": 1, "blocks": []}),
            serde_json::json!({"schema_type": "product", "product_price": "12.50", "product_currency": "EUR"}),
        );
        let doc = structured_data(&product, "u").expect("product");
        assert_eq!(doc["offers"]["priceCurrency"], "EUR");

        // A type with nothing to say says nothing, rather than an empty document.
        let empty = post(
            serde_json::json!({"schema_version": 1, "blocks": []}),
            serde_json::json!({"schema_type": "faq"}),
        );
        assert!(structured_data(&empty, "u").is_none());
        let plain = post(
            serde_json::json!({"schema_version": 1, "blocks": []}),
            serde_json::json!({}),
        );
        assert!(structured_data(&plain, "u").is_none());
    }

    #[test]
    fn strip_tags_keeps_words_only() {
        assert_eq!(strip_tags("a <b>b</b>&amp;c  <br> d"), "a b&c d");
    }
}
