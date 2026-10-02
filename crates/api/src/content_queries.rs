//! The api-side implementation of the themes `ContentQueries` seam.
//!
//! Wraps repositories/services into the trait the dynamic-block resolvers
//! consume; this is also the point where plugin hook filtering will be
//! injected (phase 33).

use vyasa_common::AppError;
use vyasa_db::content_models::Taxonomy;
use vyasa_db::content_models::{PostStatus, PostType};
use vyasa_db::repo::{CommentsRepo, PostsRepo, TermsRepo};
use vyasa_themes::{CommentNodeData, ContentQueries, MonthArchiveData, PostCardData, TermLinkData};

use std::collections::BTreeMap;

const MONTH_NAMES: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// Concrete query facade.
pub struct ApiContentQueries {
    posts: PostsRepo,
    terms: TermsRepo,
    comments: CommentsRepo,
    menus: vyasa_db::repo::MenusRepo,
    /// Embeddings and recordings for the AI-backed blocks.
    ai_data: vyasa_db::repo::AiDataRepo,
    /// Feature switches.
    options: vyasa_db::repo::OptionsRepo,
    /// Which custom types are currently served. A binding to a type whose
    /// plugin is disabled resolves to nothing, matching the router: the
    /// rows survive, they just stop being shown.
    plugin_surface: std::sync::Arc<crate::plugin_surface::PluginSurface>,
    /// Administrators' content types and field definitions.
    pool: sqlx::PgPool,
}

impl std::fmt::Debug for ApiContentQueries {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ApiContentQueries")
    }
}

impl ApiContentQueries {
    /// Builds the facade from repos already present in [`crate::state::AppState`].
    #[must_use]
    #[allow(clippy::too_many_arguments)] // one per repository it reads
    pub fn new(
        posts: PostsRepo,
        terms: TermsRepo,
        comments: CommentsRepo,
        menus: vyasa_db::repo::MenusRepo,
        ai_data: vyasa_db::repo::AiDataRepo,
        options: vyasa_db::repo::OptionsRepo,
        plugin_surface: std::sync::Arc<crate::plugin_surface::PluginSurface>,
        pool: sqlx::PgPool,
    ) -> Self {
        Self {
            posts,
            terms,
            comments,
            menus,
            ai_data,
            options,
            plugin_surface,
            pool,
        }
    }

    async fn presentation(&self) -> (vyasa_core::options::PermalinkPattern, chrono_tz::Tz, String) {
        let service = vyasa_core::options::OptionsService::new(self.options.clone());
        let pattern = service.permalink_pattern().await.unwrap_or_else(|_| {
            vyasa_core::options::PermalinkPattern(
                vyasa_core::options::PermalinkPattern::DEFAULT.into(),
            )
        });
        let zone = service
            .string_option("timezone")
            .await
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(chrono_tz::UTC);
        let format = service
            .string_option("date_format")
            .await
            .ok()
            .filter(|s| crate::public::page_meta::valid_date_format(s))
            .unwrap_or_else(|| "%b %e, %Y".into());
        (pattern, zone, format)
    }

    async fn entries_impl(
        &self,
        source: &str,
        sort: vyasa_themes::EntrySort,
        limit: u32,
        term: Option<&str>,
    ) -> Result<Vec<PostCardData>, String> {
        let bind = vyasa_themes::Binding {
            source: source.to_owned(),
            term: term.map(str::to_owned),
            sort,
            field_sort: None,
            filters: Vec::new(),
            limit,
        };
        self.bound_impl(&bind).await
    }

    async fn bound_impl(&self, bind: &vyasa_themes::Binding) -> Result<Vec<PostCardData>, String> {
        // An unknown type is not an error: a binding survives its plugin
        // (or a deleted content type) the way stored entries do, and
        // renders as an empty section until the type returns. Same for a
        // term that no longer exists.
        let Ok(post_type) = PostType::parse(&bind.source) else {
            return Ok(Vec::new());
        };
        // Only types the public may see: never patterns, and a custom
        // type only while it is served publicly — the router's rule. The
        // interned name outlives its owner (stored rows must stay
        // decodable), so registration alone is not the test.
        if !crate::policy::publicly_openable(&self.plugin_surface, &self.pool, post_type).await {
            return Ok(Vec::new());
        }
        let term_id = match bind.term.as_deref() {
            Some(spec) => {
                let (taxonomy, slug) = match spec.split_once(':') {
                    Some((tax, slug)) => {
                        let Ok(taxonomy) = vyasa_db::content_models::Taxonomy::parse(tax) else {
                            return Ok(Vec::new());
                        };
                        (taxonomy, slug)
                    }
                    None => (vyasa_db::content_models::Taxonomy::Category, spec),
                };
                match self.terms.get_by_slug(taxonomy, slug).await {
                    Ok(term) => Some(term.id),
                    Err(_) => return Ok(Vec::new()),
                }
            }
            None => None,
        };
        let sort = match bind.sort {
            vyasa_themes::EntrySort::Newest => vyasa_db::repo::PostSort::Newest,
            vyasa_themes::EntrySort::Oldest => vyasa_db::repo::PostSort::Oldest,
            vyasa_themes::EntrySort::Title => vyasa_db::repo::PostSort::Title,
            vyasa_themes::EntrySort::Updated => vyasa_db::repo::PostSort::Updated,
        };
        // Field conditions and orders name fields of the type as defined
        // now. A condition on a field that no longer exists matches
        // nothing; an order by one falls back to the plain order. Neither
        // is an error: a page never fails because its binding outlived a
        // field.
        let defined: Vec<String> = if bind.filters.is_empty() && bind.field_sort.is_none() {
            Vec::new()
        } else {
            vyasa_core::content::ContentFieldsService::new(self.pool.clone())
                .definitions(post_type)
                .await
                .unwrap_or_default()
                .into_iter()
                .map(|d| d.key)
                .collect()
        };
        if bind.filters.iter().any(|(k, _)| !defined.contains(k)) {
            return Ok(Vec::new());
        }
        let field_sort = bind
            .field_sort
            .as_ref()
            .filter(|o| defined.contains(&o.key))
            .map(|o| (o.key.as_str(), o.descending));
        let rows = self
            .posts
            .list_bound(&vyasa_db::repo::BoundQuery {
                post_type,
                term_id,
                sort,
                field_sort,
                filters: &bind.filters,
                limit: bind.limit.clamp(1, vyasa_themes::MAX_BOUND_ENTRIES),
            })
            .await
            .map_err(|e| e.to_string())?;
        let (pattern, zone, date_format) = self.presentation().await;
        Ok(rows
            .into_iter()
            .map(|p| PostCardData {
                id: p.id,
                title: p.title.clone(),
                url: pattern.path_for(&p),
                excerpt: p.excerpt.clone().unwrap_or_default(),
                thumb_url: p
                    .meta
                    .get("featured_media_url")
                    .and_then(|v| v.as_str())
                    .map(str::to_owned),
                blurhash: p
                    .meta
                    .get("featured_blurhash")
                    .and_then(|v| v.as_str())
                    .map(str::to_owned),
                thumb_focal: p
                    .meta
                    .get("featured_focal")
                    .and_then(|v| v.as_str())
                    .map(str::to_owned),
                date: p.published_at.map_or_else(String::new, |d| {
                    d.with_timezone(&zone).format(&date_format).to_string()
                }),
                author: String::new(), // author join deferred to phase 40 cards
            })
            .collect())
    }
}

fn build_page_tree(
    parent: Option<i64>,
    depth: u8,
    by_parent: &std::collections::BTreeMap<Option<i64>, Vec<vyasa_db::content_models::PostRow>>,
    out: &mut Vec<vyasa_themes::PageNodeData>,
) {
    if depth > 3 {
        return;
    }
    for p in by_parent.get(&parent).into_iter().flatten() {
        out.push(vyasa_themes::PageNodeData {
            id: p.id,
            title: p.title.clone(),
            url: format!("/{}", p.slug),
            depth,
        });
        build_page_tree(Some(p.id), depth + 1, by_parent, out);
    }
}

impl ContentQueries for ApiContentQueries {
    fn entries<'a>(
        &'a self,
        source: &'a str,
        sort: vyasa_themes::EntrySort,
        limit: u32,
        term: Option<&'a str>,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Vec<PostCardData>, String>> + Send + 'a>,
    > {
        Box::pin(self.entries_impl(source, sort, limit, term))
    }

    fn bound_entries<'a>(
        &'a self,
        bind: &'a vyasa_themes::Binding,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Vec<PostCardData>, String>> + Send + 'a>,
    > {
        Box::pin(self.bound_impl(bind))
    }

    fn categories<'a>(
        &'a self,
        show_counts: bool,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Vec<TermLinkData>, String>> + Send + 'a>,
    > {
        Box::pin(async move {
            let rows = self
                .terms
                .list(Some(Taxonomy::Category), true)
                .await
                .unwrap_or_default();
            Ok(rows
                .into_iter()
                .map(|wc| TermLinkData {
                    name: wc.term.name,
                    url: format!("/category/{}", wc.term.slug),
                    count: if show_counts {
                        Some(wc.post_count)
                    } else {
                        None
                    },
                })
                .collect())
        })
    }

    fn popular_tags<'a>(
        &'a self,
        count: u32,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Vec<TermLinkData>, String>> + Send + 'a>,
    > {
        Box::pin(async move {
            let rows = self
                .terms
                .list(Some(Taxonomy::Tag), true)
                .await
                .unwrap_or_default();
            let mut links: Vec<TermLinkData> = rows
                .into_iter()
                .map(|wc| TermLinkData {
                    name: wc.term.name,
                    url: format!("/tag/{}", wc.term.slug),
                    count: Some(wc.post_count),
                })
                .collect();
            links.sort_by_key(|l| std::cmp::Reverse(l.count));
            links.truncate(count as usize);
            Ok(links)
        })
    }

    fn monthly_archives<'a>(
        &'a self,
        months: u32,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Vec<MonthArchiveData>, String>> + Send + 'a>,
    > {
        Box::pin(async move {
            let raw = self
                .posts
                .month_counts(months)
                .await
                .map_err(|e: AppError| e.to_string())?;
            Ok(raw
                .into_iter()
                .map(|(year, month, count)| MonthArchiveData {
                    label: format!("{} {year}", MONTH_NAMES[(month as usize - 1).min(11)]),
                    url: format!("/archive/{year}/{month:02}"),
                    count,
                })
                .collect())
        })
    }

    fn navigation_menu<'a>(
        &'a self,
        slug: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + 'a>>
    {
        Box::pin(async move {
            let menu = self.menus.get_by_slug(slug).await;
            match menu {
                Ok(menu) => {
                    let items = self.menus.items(menu.id).await.map_err(|e| e.to_string())?;
                    Ok(vyasa_core::menu::MenuDraft::from_rows(&menu, &items).render_html())
                }
                Err(AppError::NotFound { .. }) => Ok(String::new()),
                Err(e) => Err(e.to_string()),
            }
        })
    }

    fn page_tree<'a>(
        &'a self,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<Vec<vyasa_themes::PageNodeData>, String>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(async move {
            // Pages with parent ordering from the posts table.
            let filter = vyasa_db::repo::PostFilter {
                status: Some(PostStatus::Published),
                post_type: Some(vyasa_db::content_models::PostType::Page),
                limit: 200,
                ..vyasa_db::repo::PostFilter::default()
            };
            let rows = self.posts.list(&filter).await.map_err(|e| e.to_string())?;
            let mut by_parent: std::collections::BTreeMap<Option<i64>, Vec<_>> =
                std::collections::BTreeMap::new();
            for p in &rows {
                by_parent
                    .entry(p.parent_id)
                    .or_insert_with(Vec::new)
                    .push(vyasa_db::content_models::PostRow::clone(p));
            }
            let mut out = Vec::new();
            build_page_tree(None, 1, &by_parent, &mut out);
            Ok(out)
        })
    }

    fn related_posts<'a>(
        &'a self,
        post_id: i64,
        count: u32,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Vec<PostCardData>, String>> + Send + 'a>,
    > {
        Box::pin(async move {
            let on = self
                .options
                .get_or_default("ai_related_posts", serde_json::Value::Bool(false))
                .await
                .ok()
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if !on {
                return Ok(Vec::new());
            }
            let limit = usize::try_from(count.clamp(1, 10)).unwrap_or(5);
            // Only entries the public may open: no protected entry, no
            // block, no type that is not served publicly.
            let public_custom =
                crate::policy::PublicTypes::load(&self.plugin_surface, &self.pool).await;
            let slugs: Vec<String> = public_custom.slugs();
            let ids = crate::ai_features::related_ids(&self.ai_data, post_id, limit, &slugs)
                .await
                .map_err(|e| e.to_string())?;
            let (pattern, zone, date_format) = self.presentation().await;
            let mut cards = Vec::with_capacity(ids.len());
            for id in ids {
                if let Ok(p) = self.posts.get(id).await {
                    if p.status == PostStatus::Published
                        && p.password_hash.is_none()
                        && public_custom.opens(p.post_type)
                    {
                        cards.push(PostCardData {
                            id: p.id,
                            title: p.title.clone(),
                            url: pattern.path_for(&p),
                            excerpt: p.excerpt.clone().unwrap_or_default(),
                            thumb_url: None,
                            thumb_focal: None,
                            blurhash: None,
                            date: p.published_at.map_or_else(String::new, |d| {
                                d.with_timezone(&zone).format(&date_format).to_string()
                            }),
                            author: String::new(),
                        });
                    }
                }
            }
            Ok(cards)
        })
    }

    fn post_audio_url<'a>(
        &'a self,
        post_id: i64,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Option<String>, String>> + Send + 'a>,
    > {
        Box::pin(async move {
            let row = self
                .ai_data
                .post_audio(post_id)
                .await
                .map_err(|e| e.to_string())?;
            Ok(row.map(|a| format!("/api/v1/media/{}/raw", a.media_id)))
        })
    }

    fn approved_comments<'a>(
        &'a self,
        post_id: i64,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Vec<CommentNodeData>, String>> + Send + 'a>,
    > {
        Box::pin(async move {
            let rows = self
                .comments
                .list_approved_threaded(post_id)
                .await
                .map_err(|e| e.to_string())?;
            let (_, zone, date_format) = self.presentation().await;
            Ok(thread_from_rows(&rows, zone, &date_format))
        })
    }
}

fn build_children(
    parent: Option<i64>,
    children: &BTreeMap<Option<i64>, Vec<i64>>,
    by_id: &BTreeMap<i64, vyasa_db::content_models::CommentRow>,
    zone: chrono_tz::Tz,
    date_format: &str,
) -> Vec<CommentNodeData> {
    children
        .get(&parent)
        .map(|ids| {
            ids.iter()
                .filter_map(|id| by_id.get(id))
                .map(|c| CommentNodeData {
                    id: c.id,
                    author: c.author_name.clone(),
                    date: c
                        .created_at
                        .with_timezone(&zone)
                        .format(date_format)
                        .to_string(),
                    html: vyasa_themes::sanitize_comment_html(&c.content),
                    children: build_children(Some(c.id), children, by_id, zone, date_format),
                })
                .collect()
        })
        .unwrap_or_default()
}

fn thread_from_rows(
    rows: &[vyasa_db::content_models::CommentRow],
    zone: chrono_tz::Tz,
    date_format: &str,
) -> Vec<CommentNodeData> {
    // Rows arrive depth-first with parent links; rebuild the nested tree.
    let by_id: BTreeMap<i64, vyasa_db::content_models::CommentRow> =
        rows.iter().map(|c| (c.id, c.clone())).collect();
    let mut children: BTreeMap<Option<i64>, Vec<i64>> = BTreeMap::new();
    for c in rows {
        children.entry(c.parent_id).or_default().push(c.id);
    }
    build_children(None, &children, &by_id, zone, date_format)
}
