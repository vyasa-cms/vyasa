//! The agents' eyes: rendering a candidate and reading the page back.
//!
//! The toolboxes in `vyasa-ai` are pure state; this is the seam where
//! they borrow the server — the same `render_candidate` /
//! `render_entry_candidate` paths the studio previews use, digested into
//! something a model can read: the section order and the visible text,
//! not a wall of markup.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use vyasa_core::menu::MenuDraft;
use vyasa_themes::{page_digest, ContentQueries, DraftState, EntrySort, Section};

use crate::state::AppState;

/// The live queries with the agent's staged menus in front: a `look`
/// shows the navigation the proposal would produce, everything else
/// stays the site's real content.
struct MenuOverlayQueries {
    inner: Arc<dyn ContentQueries>,
    /// Slug → pre-rendered menu HTML (the one shared renderer).
    staged: HashMap<String, String>,
}

impl ContentQueries for MenuOverlayQueries {
    fn entries<'a>(
        &'a self,
        source: &'a str,
        sort: EntrySort,
        limit: u32,
        term: Option<&'a str>,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<vyasa_themes::PostCardData>, String>> + Send + 'a>>
    {
        self.inner.entries(source, sort, limit, term)
    }
    fn bound_entries<'a>(
        &'a self,
        bind: &'a vyasa_themes::Binding,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<vyasa_themes::PostCardData>, String>> + Send + 'a>>
    {
        self.inner.bound_entries(bind)
    }
    fn categories<'a>(
        &'a self,
        show_counts: bool,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<vyasa_themes::TermLinkData>, String>> + Send + 'a>>
    {
        self.inner.categories(show_counts)
    }
    fn popular_tags<'a>(
        &'a self,
        count: u32,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<vyasa_themes::TermLinkData>, String>> + Send + 'a>>
    {
        self.inner.popular_tags(count)
    }
    fn monthly_archives<'a>(
        &'a self,
        months: u32,
    ) -> Pin<
        Box<dyn Future<Output = Result<Vec<vyasa_themes::MonthArchiveData>, String>> + Send + 'a>,
    > {
        self.inner.monthly_archives(months)
    }
    fn navigation_menu<'a>(
        &'a self,
        slug: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>> {
        if let Some(html) = self.staged.get(slug) {
            let html = html.clone();
            return Box::pin(async move { Ok(html) });
        }
        self.inner.navigation_menu(slug)
    }
    fn page_tree<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<vyasa_themes::PageNodeData>, String>> + Send + 'a>>
    {
        self.inner.page_tree()
    }
    fn related_posts<'a>(
        &'a self,
        post_id: i64,
        count: u32,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<vyasa_themes::PostCardData>, String>> + Send + 'a>>
    {
        self.inner.related_posts(post_id, count)
    }
    fn tree<'a>(
        &'a self,
        post_type: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<vyasa_themes::TreeNodeData>, String>> + Send + 'a>>
    {
        self.inner.tree(post_type)
    }
    fn entry_context<'a>(
        &'a self,
        post_id: i64,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<Option<vyasa_themes::EntryContextData>, String>> + Send + 'a,
        >,
    > {
        self.inner.entry_context(post_id)
    }
    fn post_audio_url<'a>(
        &'a self,
        post_id: i64,
    ) -> Pin<Box<dyn Future<Output = Result<Option<String>, String>> + Send + 'a>> {
        self.inner.post_audio_url(post_id)
    }
    fn approved_comments<'a>(
        &'a self,
        post_id: i64,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<vyasa_themes::CommentNodeData>, String>> + Send + 'a>>
    {
        self.inner.approved_comments(post_id)
    }
}

/// Live eyes for the studio agent.
pub struct LiveStudioEyes {
    /// Version supplying the draft's bundled images and fonts.
    pub base_theme_id: Option<i64>,
    /// The server, for candidate renders and content lists.
    pub state: AppState,
}

impl vyasa_ai::theme_studio::StudioEyes for LiveStudioEyes {
    fn look<'a>(
        &'a mut self,
        sandbox: &'a DraftState,
        menus: &'a [MenuDraft],
        path: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + 'a>>
    {
        Box::pin(async move {
            // Staged menus stand in for live ones during the render, so
            // the digest shows the nav the proposal would produce.
            let mut state = self.state.clone();
            if !menus.is_empty() {
                state.content_queries = Arc::new(MenuOverlayQueries {
                    inner: Arc::clone(&self.state.content_queries),
                    staged: menus
                        .iter()
                        .map(|m| (m.slug.clone(), m.render_html()))
                        .collect(),
                });
            }
            let html = crate::public::routes::render_candidate(
                &state,
                &sandbox.tokens_json(),
                &sandbox.layout_json(),
                sandbox.templates_json().as_ref(),
                sandbox.assets_json().as_ref(),
                path,
                self.base_theme_id,
            )
            .await?;
            Ok(page_digest(&html))
        })
    }

    fn content<'a>(
        &'a mut self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + 'a>>
    {
        Box::pin(async move {
            let filter = vyasa_db::repo::PostFilter {
                status: Some(vyasa_db::content_models::PostStatus::Published),
                limit: 12,
                readable_types: crate::policy::readable_custom_types(&self.state, None).await,
                ..vyasa_db::repo::PostFilter::default()
            };
            let rows = self
                .state
                .posts
                .list(&filter)
                .await
                .map_err(|e| e.to_string())?;
            if rows.is_empty() {
                return Ok("No published content yet.".to_owned());
            }
            Ok(rows
                .iter()
                .map(|p| format!("- {} ({})", p.title, p.post_type.as_str()))
                .collect::<Vec<_>>()
                .join("\n"))
        })
    }
}

/// Live eyes for the page agent: one post, rendered with the sandbox tree
/// over its unsaved content.
pub struct LivePageEyes {
    /// The server.
    pub state: AppState,
    /// The page being composed.
    pub post: vyasa_db::content_models::PostRow,
    /// The block document as the editor holds it.
    pub content: serde_json::Value,
}

impl vyasa_ai::page_designer::PageEyes for LivePageEyes {
    fn look<'a>(
        &'a mut self,
        sandbox: &'a [Section],
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + 'a>>
    {
        Box::pin(async move {
            let html = crate::public::routes::render_entry_candidate(
                &self.state,
                &self.post,
                sandbox.to_vec(),
                &self.content,
            )
            .await?;
            Ok(page_digest(&html))
        })
    }
}
