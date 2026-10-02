//! Content export and import: a Vyasa JSON archive that round-trips
//! through `POST /api/v1/import`, and a WordPress WXR rendering of the
//! same archive that WordPress's own importer accepts.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use vyasa_common::AppError;
use vyasa_core::user::Capability;
use vyasa_db::content_models::{CommentStatus, PostStatus, PostType, Taxonomy};

use crate::middleware::Principal;
use crate::policy;
use crate::rest::roles::known_caps;
use crate::state::AppState;

/// The archive's wire version.
pub const ARCHIVE_VERSION: u32 = 1;

/// Everything an export carries. Ids are the source site's; imports
/// remap them by slug and keep the source id in meta for idempotence.
#[derive(Serialize, Deserialize, Default)]
pub struct Archive {
    pub version: u32,
    pub exported_at: String,
    pub site: serde_json::Map<String, serde_json::Value>,
    /// Custom roles. Absent from archives older than custom roles.
    #[serde(default)]
    pub roles: Vec<ArchiveRole>,
    pub users: Vec<ArchiveUser>,
    pub terms: Vec<ArchiveTerm>,
    pub posts: Vec<ArchivePost>,
    pub comments: Vec<ArchiveComment>,
    pub media: Vec<ArchiveMedia>,
    pub menus: Vec<ArchiveMenu>,
    /// Administrators' content types. Absent from archives older than
    /// content types.
    #[serde(default)]
    pub content_types: Vec<ArchiveContentType>,
    /// Field definitions on any type. Absent from older archives.
    #[serde(default)]
    pub content_fields: Vec<ArchiveField>,
}

/// An administrator's content type.
#[derive(Serialize, Deserialize)]
pub struct ArchiveContentType {
    pub slug: String,
    pub singular: String,
    pub plural: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "yes")]
    pub public: bool,
    #[serde(default = "yes")]
    pub has_archive: bool,
}

const fn yes() -> bool {
    true
}

/// A field definition, in order within its type.
#[derive(Serialize, Deserialize)]
pub struct ArchiveField {
    pub type_slug: String,
    pub key: String,
    pub label: String,
    #[serde(default)]
    pub help: String,
    pub kind: String,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub options: serde_json::Value,
}

#[derive(Serialize, Deserialize)]
pub struct ArchiveUser {
    pub id: i64,
    pub email: String,
    pub username: String,
    pub display_name: String,
    pub role: String,
    /// The slug of the user's custom role; `role` is then `subscriber`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_role: Option<String>,
    pub bio: String,
}

/// A custom role. The built-in roles are not in the archive.
#[derive(Serialize, Deserialize)]
pub struct ArchiveRole {
    pub slug: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub capabilities: Vec<String>,
}

#[derive(Serialize, Deserialize)]
pub struct ArchiveTerm {
    pub id: i64,
    pub taxonomy: String,
    pub name: String,
    pub slug: String,
    pub parent_id: Option<i64>,
}

#[derive(Serialize, Deserialize)]
pub struct ArchivePost {
    pub id: i64,
    #[serde(rename = "type")]
    pub post_type: String,
    pub status: String,
    pub slug: String,
    pub title: String,
    /// The block document.
    pub content: serde_json::Value,
    #[serde(default)]
    pub layout: Option<serde_json::Value>,
    #[serde(default)]
    pub excerpt: Option<String>,
    pub author_id: i64,
    #[serde(default)]
    pub parent_id: Option<i64>,
    #[serde(default)]
    pub meta: serde_json::Value,
    #[serde(default)]
    pub published_at: Option<String>,
    #[serde(default)]
    pub scheduled_for: Option<String>,
    #[serde(default)]
    pub term_ids: Vec<i64>,
    /// Field values (`meta` no longer carries them). Entry references are
    /// the source site's ids, remapped on import.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub fields: serde_json::Map<String, serde_json::Value>,
}

#[derive(Serialize, Deserialize)]
pub struct ArchiveComment {
    pub id: i64,
    pub post_id: i64,
    #[serde(default)]
    pub parent_id: Option<i64>,
    pub author_name: String,
    pub author_email: String,
    pub content: String,
    pub status: String,
    pub created_at: String,
}

#[derive(Serialize, Deserialize)]
pub struct ArchiveMedia {
    pub id: i64,
    pub file_name: String,
    pub mime: String,
    pub byte_size: i64,
    #[serde(default)]
    pub width: Option<i32>,
    #[serde(default)]
    pub height: Option<i32>,
    #[serde(default)]
    pub alt: Option<String>,
    #[serde(default)]
    pub caption: Option<String>,
    /// Where the bytes can be fetched from the exporting site.
    pub url: String,
}

#[derive(Serialize, Deserialize)]
pub struct ArchiveMenu {
    pub slug: String,
    pub name: String,
    #[serde(default)]
    pub location: Option<String>,
    pub items: Vec<ArchiveMenuItem>,
}

#[derive(Serialize, Deserialize)]
pub struct ArchiveMenuItem {
    pub label: String,
    pub url: String,
    #[serde(default)]
    pub children: Vec<ArchiveMenuItem>,
}

/// Options that never leave the site: secrets, progress, keys.
const PRIVATE_OPTIONS: &[&str] = &[
    "smtp_password",
    "setup_progress",
    "update_trusted_keys",
    "registry_trusted_keys",
];

/// Builds the archive from the live database.
///
/// # Errors
/// Database errors.
#[allow(clippy::too_many_lines, clippy::items_after_statements)]
pub async fn build(state: &AppState, site_url: &str) -> Result<Archive, AppError> {
    let mut site = serde_json::Map::new();
    for key in vyasa_core::options::SITE_OPTION_KEYS {
        if PRIVATE_OPTIONS.contains(key) {
            continue;
        }
        if let Ok(v) = state.options.get(key).await {
            if !v.is_null() {
                site.insert((*key).to_owned(), v);
            }
        }
    }

    let users = vyasa_db::repo::UsersRepo::new(state.pool.clone())
        .list(10_000, 0)
        .await?
        .into_iter()
        .map(|u| ArchiveUser {
            id: u.id,
            email: u.email,
            username: u.username,
            display_name: u.display_name,
            role: u.role.as_str().to_owned(),
            custom_role: u.custom_role,
            bio: u.bio,
        })
        .collect();
    let roles = state
        .roles
        .list()
        .await?
        .into_iter()
        .map(|(role, _)| ArchiveRole {
            slug: role.slug,
            name: role.name,
            description: role.description,
            capabilities: role.capabilities,
        })
        .collect();

    let terms_repo = vyasa_db::repo::TermsRepo::new(state.pool.clone());
    let terms: Vec<ArchiveTerm> = terms_repo
        .list(None, false)
        .await?
        .into_iter()
        .map(|t| ArchiveTerm {
            id: t.term.id,
            taxonomy: t.term.taxonomy.as_str().to_owned(),
            name: t.term.name,
            slug: t.term.slug,
            parent_id: t.term.parent_id,
        })
        .collect();

    let posts_repo = vyasa_db::repo::PostsRepo::new(state.pool.clone());
    let mut posts = Vec::new();
    for status in [
        PostStatus::Published,
        PostStatus::Draft,
        PostStatus::Scheduled,
        PostStatus::Private,
    ] {
        let rows = posts_repo
            .list(&vyasa_db::repo::PostFilter {
                status: Some(status),
                limit: 10_000,
                ..Default::default()
            })
            .await?;
        for p in rows {
            let term_ids = terms_repo.list_for_post(p.id).await.unwrap_or_default();
            posts.push(ArchivePost {
                id: p.id,
                post_type: p.post_type.as_str().to_owned(),
                status: p.status.as_str().to_owned(),
                slug: p.slug,
                title: p.title,
                content: p.content,
                layout: p.layout,
                excerpt: p.excerpt,
                author_id: p.author_id,
                parent_id: p.parent_id,
                fields: crate::entry_fields::stored(&p.meta),
                meta: crate::entry_fields::meta_without_fields(p.meta),
                published_at: p.published_at.map(|d| d.to_rfc3339()),
                scheduled_for: p.scheduled_for.map(|d| d.to_rfc3339()),
                term_ids,
            });
        }
    }

    let comments_repo = vyasa_db::repo::CommentsRepo::new(state.pool.clone());
    let mut comments = Vec::new();
    for status in [CommentStatus::Approved, CommentStatus::Pending] {
        for c in comments_repo.list_by_status(status, 100_000, 0).await? {
            comments.push(ArchiveComment {
                id: c.id,
                post_id: c.post_id,
                parent_id: c.parent_id,
                author_name: c.author_name,
                author_email: c.author_email,
                content: c.content,
                status: c.status.as_str().to_owned(),
                created_at: c.created_at.to_rfc3339(),
            });
        }
    }

    let media = vyasa_db::repo::MediaRepo::new(state.pool.clone())
        .list(100_000, 0)
        .await?
        .into_iter()
        .map(|m| ArchiveMedia {
            url: format!("{site_url}/api/v1/media/{}/raw", m.id),
            id: m.id,
            file_name: m.file_name,
            mime: m.mime,
            byte_size: m.byte_size,
            width: m.width,
            height: m.height,
            alt: m.alt,
            caption: m.caption,
        })
        .collect();

    let mut menus = Vec::new();
    for m in state.menus.list().await? {
        let rows = state.menus.items(m.id).await.unwrap_or_default();
        let draft = vyasa_core::menu::MenuDraft::from_rows(&m, &rows);
        fn lift(items: Vec<vyasa_core::menu::MenuDraftItem>) -> Vec<ArchiveMenuItem> {
            items
                .into_iter()
                .map(|i| ArchiveMenuItem {
                    label: i.label,
                    url: i.url,
                    children: lift(i.children),
                })
                .collect()
        }
        menus.push(ArchiveMenu {
            slug: m.slug,
            name: m.name,
            location: m.location,
            items: lift(draft.items),
        });
    }

    let content_types = vyasa_core::content::ContentTypesService::new(state.pool.clone())
        .list()
        .await?
        .into_iter()
        .map(|t| ArchiveContentType {
            slug: t.slug,
            singular: t.singular,
            plural: t.plural,
            description: t.description,
            public: t.public,
            has_archive: t.has_archive,
        })
        .collect();
    let content_fields = vyasa_db::repo::ContentFieldsRepo::new(state.pool.clone())
        .list_all()
        .await?
        .into_iter()
        .map(|f| ArchiveField {
            type_slug: f.type_slug,
            key: f.key,
            label: f.label,
            help: f.help,
            kind: f.kind,
            required: f.required,
            options: f.options,
        })
        .collect();

    Ok(Archive {
        version: ARCHIVE_VERSION,
        content_types,
        content_fields,
        exported_at: chrono::Utc::now().to_rfc3339(),
        site,
        roles,
        users,
        terms,
        posts,
        comments,
        media,
        menus,
    })
}

fn xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn cdata(s: &str) -> String {
    format!("<![CDATA[{}]]>", s.replace("]]>", "]]]]><![CDATA[>"))
}

fn wp_date(rfc: Option<&str>) -> String {
    rfc.and_then(|r| chrono::DateTime::parse_from_rfc3339(r).ok())
        .map_or_else(String::new, |d| d.format("%Y-%m-%d %H:%M:%S").to_string())
}

/// Renders the archive as WXR 1.2, which WordPress's importer reads:
/// authors, categories and tags, posts and pages with their blocks
/// rendered to HTML, comments, and attachments by URL.
#[must_use]
#[allow(clippy::too_many_lines, clippy::format_push_string)] // one document, written top to bottom
pub fn to_wxr(archive: &Archive, site_url: &str) -> String {
    let title = archive
        .site
        .get("site_title")
        .and_then(|v| v.as_str())
        .unwrap_or("Vyasa");
    let tagline = archive
        .site
        .get("site_tagline")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let mut out = String::with_capacity(64 * 1024);
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    out.push_str("<rss version=\"2.0\" xmlns:excerpt=\"http://wordpress.org/export/1.2/excerpt/\" xmlns:content=\"http://purl.org/rss/1.0/modules/content/\" xmlns:wfw=\"http://wellformedweb.org/CommentAPI/\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\" xmlns:wp=\"http://wordpress.org/export/1.2/\">\n<channel>\n");
    out.push_str(&format!(
        "<title>{}</title>\n<link>{}</link>\n<description>{}</description>\n<language>{}</language>\n<wp:wxr_version>1.2</wp:wxr_version>\n<wp:base_site_url>{}</wp:base_site_url>\n<wp:base_blog_url>{}</wp:base_blog_url>\n",
        xml(title),
        xml(site_url),
        xml(tagline),
        xml(archive.site.get("site_language").and_then(|v| v.as_str()).unwrap_or("en")),
        xml(site_url),
        xml(site_url)
    ));
    for u in &archive.users {
        out.push_str(&format!(
            "<wp:author><wp:author_id>{}</wp:author_id><wp:author_login>{}</wp:author_login><wp:author_email>{}</wp:author_email><wp:author_display_name>{}</wp:author_display_name></wp:author>\n",
            u.id,
            cdata(&u.username),
            cdata(&u.email),
            cdata(&u.display_name)
        ));
    }
    let by_id: HashMap<i64, &ArchiveTerm> = archive.terms.iter().map(|t| (t.id, t)).collect();
    for t in &archive.terms {
        let parent = t
            .parent_id
            .and_then(|p| by_id.get(&p))
            .map_or("", |p| p.slug.as_str());
        if t.taxonomy == "category" {
            out.push_str(&format!(
                "<wp:category><wp:term_id>{}</wp:term_id><wp:category_nicename>{}</wp:category_nicename><wp:category_parent>{}</wp:category_parent><wp:cat_name>{}</wp:cat_name></wp:category>\n",
                t.id, cdata(&t.slug), cdata(parent), cdata(&t.name)
            ));
        } else {
            out.push_str(&format!(
                "<wp:tag><wp:term_id>{}</wp:term_id><wp:tag_slug>{}</wp:tag_slug><wp:tag_name>{}</wp:tag_name></wp:tag>\n",
                t.id, cdata(&t.slug), cdata(&t.name)
            ));
        }
    }
    let author_login: HashMap<i64, &str> = archive
        .users
        .iter()
        .map(|u| (u.id, u.username.as_str()))
        .collect();
    for p in &archive.posts {
        let html = serde_json::from_value::<vyasa_core::BlockDocument>(p.content.clone())
            .ok()
            .and_then(|doc| vyasa_themes::renderer::blocks::render_blocks(&doc.blocks).ok())
            .unwrap_or_default();
        let status = match p.status.as_str() {
            "published" => "publish",
            "scheduled" => "future",
            "private" => "private",
            _ => "draft",
        };
        let kind = if p.post_type == "page" {
            "page"
        } else {
            "post"
        };
        let link = format!("{site_url}/{}", p.slug);
        out.push_str("<item>\n");
        out.push_str(&format!(
            "<title>{}</title>\n<link>{}</link>\n<dc:creator>{}</dc:creator>\n<guid isPermaLink=\"false\">{}</guid>\n<content:encoded>{}</content:encoded>\n<excerpt:encoded>{}</excerpt:encoded>\n<wp:post_id>{}</wp:post_id>\n<wp:post_date>{}</wp:post_date>\n<wp:post_date_gmt>{}</wp:post_date_gmt>\n<wp:status>{}</wp:status>\n<wp:post_parent>{}</wp:post_parent>\n<wp:post_type>{}</wp:post_type>\n<wp:post_name>{}</wp:post_name>\n<wp:comment_status>open</wp:comment_status>\n",
            cdata(&p.title),
            xml(&link),
            cdata(author_login.get(&p.author_id).copied().unwrap_or("admin")),
            xml(&format!("{site_url}/?p={}", p.id)),
            cdata(&html),
            cdata(p.excerpt.as_deref().unwrap_or("")),
            p.id,
            wp_date(p.published_at.as_deref().or(p.scheduled_for.as_deref())),
            wp_date(p.published_at.as_deref().or(p.scheduled_for.as_deref())),
            status,
            p.parent_id.unwrap_or(0),
            kind,
            cdata(&p.slug)
        ));
        for tid in &p.term_ids {
            if let Some(t) = by_id.get(tid) {
                let domain = if t.taxonomy == "category" {
                    "category"
                } else {
                    "post_tag"
                };
                out.push_str(&format!(
                    "<category domain=\"{domain}\" nicename=\"{}\">{}</category>\n",
                    xml(&t.slug),
                    cdata(&t.name)
                ));
            }
        }
        if let Some(obj) = p.meta.as_object() {
            for (k, v) in obj {
                if k.starts_with("import:") {
                    continue;
                }
                let value = match v {
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                out.push_str(&format!(
                    "<wp:postmeta><wp:meta_key>{}</wp:meta_key><wp:meta_value>{}</wp:meta_value></wp:postmeta>\n",
                    cdata(k),
                    cdata(&value)
                ));
            }
        }
        for c in archive.comments.iter().filter(|c| c.post_id == p.id) {
            out.push_str(&format!(
                "<wp:comment><wp:comment_id>{}</wp:comment_id><wp:comment_author>{}</wp:comment_author><wp:comment_author_email>{}</wp:comment_author_email><wp:comment_date>{}</wp:comment_date><wp:comment_date_gmt>{}</wp:comment_date_gmt><wp:comment_content>{}</wp:comment_content><wp:comment_approved>{}</wp:comment_approved><wp:comment_parent>{}</wp:comment_parent></wp:comment>\n",
                c.id,
                cdata(&c.author_name),
                cdata(&c.author_email),
                wp_date(Some(&c.created_at)),
                wp_date(Some(&c.created_at)),
                cdata(&c.content),
                i32::from(c.status == "approved"),
                c.parent_id.unwrap_or(0)
            ));
        }
        out.push_str("</item>\n");
    }
    for m in &archive.media {
        out.push_str(&format!(
            "<item>\n<title>{}</title>\n<link>{}</link>\n<guid isPermaLink=\"false\">{}</guid>\n<wp:post_id>{}</wp:post_id>\n<wp:status>inherit</wp:status>\n<wp:post_type>attachment</wp:post_type>\n<wp:post_name>{}</wp:post_name>\n<wp:attachment_url>{}</wp:attachment_url>\n<excerpt:encoded>{}</excerpt:encoded>\n<wp:postmeta><wp:meta_key>_wp_attachment_image_alt</wp:meta_key><wp:meta_value>{}</wp:meta_value></wp:postmeta>\n</item>\n",
            cdata(&m.file_name),
            xml(&m.url),
            xml(&m.url),
            m.id,
            cdata(&m.file_name),
            xml(&m.url),
            cdata(m.caption.as_deref().unwrap_or("")),
            cdata(m.alt.as_deref().unwrap_or(""))
        ));
    }
    out.push_str("</channel>\n</rss>\n");
    out
}

/// What an import did.
#[derive(Serialize, Default, Debug, utoipa::ToSchema)]
pub struct ImportReport {
    pub roles: usize,
    pub users: usize,
    pub terms: usize,
    pub posts: usize,
    pub comments: usize,
    pub menus: usize,
    pub options: usize,
    /// Content types created.
    #[serde(default)]
    pub content_types: usize,
    /// Field definitions created.
    #[serde(default)]
    pub content_fields: usize,
    pub skipped: usize,
    pub warnings: Vec<String>,
}

/// Creates the archive's content types and field definitions that are
/// not here yet, through the same services (and rules) as the admin. An
/// existing slug or key is left as it is.
async fn import_content_model(state: &AppState, archive: &Archive, report: &mut ImportReport) {
    let types = vyasa_core::content::ContentTypesService::new(state.pool.clone());
    let plugin_slugs: Vec<String> = state
        .plugin_surface
        .post_types()
        .await
        .into_iter()
        .map(|d| d.slug)
        .collect();
    for t in &archive.content_types {
        match types
            .create(
                vyasa_core::content::NewType {
                    slug: t.slug.clone(),
                    singular: t.singular.clone(),
                    plural: t.plural.clone(),
                    description: t.description.clone(),
                    public: t.public,
                    has_archive: t.has_archive,
                },
                &plugin_slugs,
            )
            .await
        {
            Ok(_) => report.content_types += 1,
            Err(AppError::Conflict { .. }) if types.get(&t.slug).await.is_ok() => {
                report.skipped += 1;
            }
            Err(e) => report
                .warnings
                .push(format!("content type {}: {e}", t.slug)),
        }
    }
    let fields = vyasa_core::content::ContentFieldsService::new(state.pool.clone());
    for f in &archive.content_fields {
        if fields.get(&f.type_slug, &f.key).await.is_ok() {
            report.skipped += 1;
            continue;
        }
        let kind = match vyasa_core::content::FieldKind::parse(&f.kind) {
            Ok(k) => k,
            Err(e) => {
                report
                    .warnings
                    .push(format!("field {}.{}: {e}", f.type_slug, f.key));
                continue;
            }
        };
        let options = if f.options.is_null() {
            serde_json::json!({})
        } else {
            f.options.clone()
        };
        match fields
            .create(
                &f.type_slug,
                vyasa_core::content::NewField {
                    key: f.key.clone(),
                    label: f.label.clone(),
                    help: f.help.clone(),
                    kind,
                    required: f.required,
                    options,
                },
            )
            .await
        {
            Ok(_) => report.content_fields += 1,
            Err(e) => report
                .warnings
                .push(format!("field {}.{}: {e}", f.type_slug, f.key)),
        }
    }
}

/// Writes the field values of the entries this import inserted, each
/// value checked by the fields' own validation. Entry references are
/// remapped to the new ids; a value that does not pass (no such field
/// here, a reference to something this site does not have) is dropped
/// and reported, never stored unchecked.
async fn import_field_values(
    state: &AppState,
    archive: &Archive,
    inserted: &[(usize, i64, PostType)],
    post_map: &HashMap<i64, i64>,
    report: &mut ImportReport,
) {
    let fields = vyasa_core::content::ContentFieldsService::new(state.pool.clone());
    let posts_repo = vyasa_db::repo::PostsRepo::new(state.pool.clone());
    let mut defs = crate::entry_fields::Definitions::default();
    for (index, new_id, post_type) in inserted {
        let Some(p) = archive.posts.get(*index) else {
            continue;
        };
        if p.fields.is_empty() {
            continue;
        }
        let kinds: HashMap<String, vyasa_core::content::FieldKind> = defs
            .of(&state.pool, *post_type)
            .await
            .iter()
            .map(|d| (d.key.clone(), d.kind))
            .collect();
        let mut kept = serde_json::Map::new();
        for (key, value) in &p.fields {
            let value = match kinds.get(key) {
                // Media files are not copied by a JSON import, so an id
                // from the source site names nothing here (or, worse,
                // something else): dropped, and said so.
                Some(vyasa_core::content::FieldKind::Media) => {
                    report.warnings.push(format!(
                        "post {}: field {key} dropped: media references do not carry over in \
                         an import (the files are not copied); choose the media again",
                        p.slug
                    ));
                    continue;
                }
                Some(vyasa_core::content::FieldKind::Entry) => {
                    if let Some(new) = remap_entry(value, post_map) {
                        new
                    } else {
                        report.warnings.push(format!(
                            "post {}: field {key} dropped: the entry it points at is not in \
                             this archive",
                            p.slug
                        ));
                        continue;
                    }
                }
                _ => value.clone(),
            };
            let one = serde_json::json!({ key.clone(): value });
            match fields.validate_values(*post_type, &one, None).await {
                Ok(valid) => kept.extend(valid),
                Err(e) => report
                    .warnings
                    .push(format!("post {}: field value dropped: {e}", p.slug)),
            }
        }
        if kept.is_empty() {
            continue;
        }
        if let Err(e) = posts_repo
            .set_field_values(*new_id, &serde_json::Value::Object(kept))
            .await
        {
            report.warnings.push(format!("post {}: {e}", p.slug));
        }
    }
}

/// An entry reference in the archive's ids, as this site's id; `None`
/// when the archive does not carry that entry (a source id must never be
/// kept: here it would name some other entry, or none).
fn remap_entry(
    value: &serde_json::Value,
    post_map: &HashMap<i64, i64>,
) -> Option<serde_json::Value> {
    let old = match value {
        serde_json::Value::String(s) => s.parse::<i64>().ok(),
        serde_json::Value::Number(n) => n.as_i64(),
        _ => None,
    }?;
    post_map
        .get(&old)
        .map(|new| serde_json::Value::String(new.to_string()))
}

fn import_key(kind: &str, id: i64) -> String {
    format!("import:vyasa:{kind}:{id}")
}

/// Creates the archive's custom roles that are not here yet; a slug that
/// exists is left as it is. An `importer` creates a role only under the
/// rules of `POST /roles`: `manage_users`, and every capability in the
/// role held by them.
async fn import_roles(
    state: &AppState,
    archive: &Archive,
    importer: Option<&Principal>,
    report: &mut ImportReport,
) {
    for r in &archive.roles {
        if state.roles.get(&r.slug).await.is_ok() {
            report.skipped += 1;
            continue;
        }
        let allowed = importer.map_or(Ok(()), |who| {
            who.ensure(Capability::ManageUsers)?;
            policy::may_grant(who, &known_caps(&r.capabilities))
        });
        let created = match allowed {
            Ok(()) => state
                .roles
                .restore(vyasa_core::user::RoleInput {
                    slug: r.slug.clone(),
                    name: r.name.clone(),
                    description: r.description.clone(),
                    capabilities: r.capabilities.clone(),
                })
                .await
                .map(|_| ()),
            Err(e) => Err(e),
        };
        match created {
            Ok(()) => report.roles += 1,
            Err(e) => report.warnings.push(format!("role {}: {e}", r.slug)),
        }
    }
}

/// Gives a user just imported the custom role the archive names for
/// them, when the role exists here and `importer` may hand it out.
/// Otherwise they stay as created, with a warning.
async fn import_custom_role(
    state: &AppState,
    importer: Option<&Principal>,
    user_id: i64,
    slug: &str,
) -> Result<(), AppError> {
    let role = state.roles.get(slug).await?;
    if let Some(who) = importer {
        who.ensure(Capability::ManageUsers)?;
        policy::may_grant(who, &known_caps(&role.capabilities))?;
    }
    vyasa_db::repo::UsersRepo::new(state.pool.clone())
        .set_custom_role(user_id, Some(slug))
        .await
}

/// Loads an archive. Existing rows are matched by email, slug, or import
/// key and left alone, so running an import twice adds nothing.
///
/// `importer` is whoever asked over HTTP, and bounds the roles the import
/// creates and hands out; `None` is the operator at the command line,
/// who is bounded by nobody.
///
/// # Errors
/// Database errors; an archive of another version.
#[allow(clippy::too_many_lines, clippy::items_after_statements)]
pub async fn import(
    state: &AppState,
    archive: &Archive,
    options_too: bool,
    importer: Option<&Principal>,
) -> Result<ImportReport, AppError> {
    if archive.version != ARCHIVE_VERSION {
        return Err(AppError::validation(format!(
            "archive version {} is not {ARCHIVE_VERSION}",
            archive.version
        )));
    }
    let mut report = ImportReport::default();
    let pool = state.pool.clone();
    let users_repo = vyasa_db::repo::UsersRepo::new(pool.clone());
    let terms_repo = vyasa_db::repo::TermsRepo::new(pool.clone());
    let posts_repo = vyasa_db::repo::PostsRepo::new(pool.clone());
    let comments_repo = vyasa_db::repo::CommentsRepo::new(pool.clone());

    if options_too {
        for (k, v) in &archive.site {
            if PRIVATE_OPTIONS.contains(&k.as_str()) {
                continue;
            }
            match state.options_service.put(k, v.clone()).await {
                Ok(()) => report.options += 1,
                Err(e) => report.warnings.push(format!("option {k}: {e}")),
            }
        }
    }

    import_roles(state, archive, importer, &mut report).await;

    // Users by email; new ones arrive passwordless and get an invitation
    // when an administrator sends one.
    let mut user_map: HashMap<i64, i64> = HashMap::new();
    let fallback_admin = users_repo
        .list(100, 0)
        .await?
        .into_iter()
        .find(|u| u.role == vyasa_db::models::Role::Admin && u.email_verified_at.is_some())
        .map(|u| u.id);
    for u in &archive.users {
        if let Ok(existing) = users_repo.get_by_email(&u.email).await {
            // An account that has not confirmed its address is not the
            // archive's author, whatever address it registered with: its
            // posts go to the fallback administrator instead.
            if existing.email_verified_at.is_none() {
                report.warnings.push(format!(
                    "user {}: the account here with this address is not confirmed; \
                     their posts are attributed to an administrator",
                    u.email
                ));
                continue;
            }
            user_map.insert(u.id, existing.id);
            report.skipped += 1;
            continue;
        }
        let mut role =
            vyasa_db::models::Role::parse(&u.role).unwrap_or(vyasa_db::models::Role::Subscriber);
        // An importer gives no role they could not give by hand. The
        // account still arrives, so its content keeps its author, as the
        // subscriber an import has always been able to create.
        let grantable = match importer {
            Some(who) if role != vyasa_db::models::Role::Subscriber => {
                policy::may_grant(who, vyasa_core::user::role_caps(role))
            }
            _ => Ok(()),
        };
        if let Err(e) = grantable {
            report.warnings.push(format!(
                "user {}: arrives as a subscriber, not {}: {e}",
                u.email,
                role.as_str()
            ));
            role = vyasa_db::models::Role::Subscriber;
        }
        match state
            .users
            .create(vyasa_core::user::CreateUser {
                email: u.email.clone(),
                username: Some(u.username.clone()),
                display_name: Some(u.display_name.clone()),
                password: None,
                role,
            })
            .await
        {
            Ok(row) => {
                user_map.insert(u.id, row.id);
                report.users += 1;
                if let Some(slug) = &u.custom_role {
                    if let Err(e) = import_custom_role(state, importer, row.id, slug).await {
                        report.warnings.push(format!(
                            "user {}: arrives without the role {slug}: {e}",
                            u.email
                        ));
                    }
                }
            }
            Err(e) => report.warnings.push(format!("user {}: {e}", u.email)),
        }
    }

    // Terms by slug within taxonomy; parents in a second pass.
    let mut term_map: HashMap<i64, i64> = HashMap::new();
    for t in &archive.terms {
        let taxonomy = match t.taxonomy.as_str() {
            "category" => Taxonomy::Category,
            "tag" => Taxonomy::Tag,
            other => match Taxonomy::register(other) {
                Ok(t) => t,
                Err(_) => Taxonomy::Tag,
            },
        };
        if let Ok(existing) = terms_repo.get_by_slug(taxonomy, &t.slug).await {
            term_map.insert(t.id, existing.id);
            report.skipped += 1;
            continue;
        }
        match terms_repo
            .insert(&vyasa_db::repo::NewTerm {
                id: vyasa_common::next_id_i64(),
                taxonomy,
                name: &t.name,
                slug: &t.slug,
                parent_id: None,
                meta: serde_json::json!({ "import_key": import_key("term", t.id) }),
            })
            .await
        {
            Ok(row) => {
                term_map.insert(t.id, row.id);
                report.terms += 1;
            }
            Err(e) => report.warnings.push(format!("term {}: {e}", t.slug)),
        }
    }
    for t in &archive.terms {
        if let (Some(parent), Some(id)) = (
            t.parent_id.and_then(|p| term_map.get(&p)),
            term_map.get(&t.id),
        ) {
            let _ = terms_repo.set_parent(*id, Some(*parent)).await;
        }
    }

    import_content_model(state, archive, &mut report).await;

    // Posts by type and slug; parents in a second pass.
    let mut post_map: HashMap<i64, i64> = HashMap::new();
    // Entries inserted here (not matched to existing ones), whose field
    // values are written once every entry has its new id.
    let mut inserted: Vec<(usize, i64, PostType)> = Vec::new();
    for (index, p) in archive.posts.iter().enumerate() {
        let post_type = match p.post_type.as_str() {
            "page" => PostType::Page,
            "post" => PostType::Post,
            // A content type the archive brought, or one a plugin here
            // declares. Anything else lands as a post, as it always has.
            other => {
                if let Ok(t @ PostType::Custom(_)) = PostType::parse(other) {
                    t
                } else {
                    if other != "block" {
                        report.warnings.push(format!(
                            "post {}: the type {other:?} does not exist here; imported as a post",
                            p.slug
                        ));
                    }
                    PostType::Post
                }
            }
        };
        if let Ok(existing) = posts_repo.get_by_slug(post_type, &p.slug).await {
            post_map.insert(p.id, existing.id);
            report.skipped += 1;
            continue;
        }
        let Some(author) = user_map.get(&p.author_id).copied().or(fallback_admin) else {
            report
                .warnings
                .push(format!("post {}: no author available", p.slug));
            continue;
        };
        let status = match p.status.as_str() {
            "published" => PostStatus::Published,
            "scheduled" => PostStatus::Scheduled,
            "private" => PostStatus::Private,
            _ => PostStatus::Draft,
        };
        let mut meta = p.meta.clone();
        if !meta.is_object() {
            meta = serde_json::json!({});
        }
        if let Some(obj) = meta.as_object_mut() {
            // Field values never arrive through raw meta: they go through
            // the fields' validation below, from `fields`.
            if obj.remove("fields").is_some() {
                report.warnings.push(format!(
                    "post {}: meta.fields ignored; field values are imported from `fields`",
                    p.slug
                ));
            }
            obj.insert(
                "import_key".into(),
                serde_json::Value::String(import_key("post", p.id)),
            );
        }
        let parse = |s: &Option<String>| {
            s.as_deref()
                .and_then(|r| chrono::DateTime::parse_from_rfc3339(r).ok())
                .map(|d| d.with_timezone(&chrono::Utc))
        };
        match posts_repo
            .insert(&vyasa_db::repo::NewPost {
                id: vyasa_common::next_id_i64(),
                layout: p.layout.clone(),
                post_type,
                status,
                slug: p.slug.clone(),
                title: p.title.clone(),
                content: p.content.clone(),
                excerpt: p.excerpt.clone(),
                author_id: author,
                parent_id: None,
                meta,
                published_at: parse(&p.published_at),
                scheduled_for: parse(&p.scheduled_for),
                password_hash: None,
            })
            .await
        {
            Ok(row) => {
                let ids: Vec<i64> = p
                    .term_ids
                    .iter()
                    .filter_map(|t| term_map.get(t).copied())
                    .collect();
                if !ids.is_empty() {
                    let _ = terms_repo.set_post_terms(row.id, &ids).await;
                }
                post_map.insert(p.id, row.id);
                inserted.push((index, row.id, post_type));
                report.posts += 1;
            }
            Err(e) => report.warnings.push(format!("post {}: {e}", p.slug)),
        }
    }
    import_field_values(state, archive, &inserted, &post_map, &mut report).await;
    for p in &archive.posts {
        if let (Some(parent), Some(id)) = (
            p.parent_id.and_then(|x| post_map.get(&x)),
            post_map.get(&p.id),
        ) {
            let _ = sqlx::query("UPDATE posts SET parent_id = $2 WHERE id = $1")
                .bind(id)
                .bind(parent)
                .execute(&pool)
                .await;
        }
    }

    // Comments by import key; parents resolved through the map.
    let mut comment_map: HashMap<i64, i64> = HashMap::new();
    for c in &archive.comments {
        let key = import_key("comment", c.id);
        if comments_repo.has_import_key(&key).await? {
            report.skipped += 1;
            continue;
        }
        let Some(post_id) = post_map.get(&c.post_id) else {
            continue;
        };
        let parent = c.parent_id.and_then(|p| comment_map.get(&p).copied());
        match comments_repo
            .insert_imported(
                *post_id,
                parent,
                &c.author_name,
                &c.author_email,
                &c.content,
                &key,
            )
            .await
        {
            Ok(id) => {
                comment_map.insert(c.id, id);
                report.comments += 1;
            }
            Err(e) => report.warnings.push(format!("comment {}: {e}", c.id)),
        }
    }

    // Menus by slug; an existing menu is left as it is.
    let menus = vyasa_core::menu::MenuService::new(state.menus.clone());
    let existing: Vec<String> = menus.list().await?.into_iter().map(|m| m.slug).collect();
    for m in &archive.menus {
        if existing.contains(&m.slug) {
            report.skipped += 1;
            continue;
        }
        fn lower(items: &[ArchiveMenuItem]) -> Vec<vyasa_core::menu::MenuDraftItem> {
            items
                .iter()
                .map(|i| vyasa_core::menu::MenuDraftItem {
                    label: i.label.clone(),
                    url: i.url.clone(),
                    children: lower(&i.children),
                })
                .collect()
        }
        match menus
            .apply_draft(&vyasa_core::menu::MenuDraft {
                slug: m.slug.clone(),
                name: m.name.clone(),
                location: m.location.clone(),
                items: lower(&m.items),
            })
            .await
        {
            Ok(()) => report.menus += 1,
            Err(e) => report.warnings.push(format!("menu {}: {e}", m.slug)),
        }
    }

    if !archive.media.is_empty() {
        report.warnings.push(format!(
            "{} media entries carry URLs only; files are not copied by a JSON import",
            archive.media.len()
        ));
    }
    Ok(report)
}
