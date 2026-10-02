//! Data-backed implementations of every built-in dynamic block.
//!
//! Resolvers are generic over the [`ContentQueries`] trait (see
//! [`crate::dynblocks::queries`]); the api crate supplies the concrete
//! implementation. Output is theme-token-classed HTML.

use std::future::Future;
use std::pin::Pin;

use super::queries::{CommentNodeData, EntrySort, PostCardData};
use super::{BlockPayload, ResolveContext};
use crate::binding::Binding;
use crate::layout::MapSettings;
use crate::renderer::esc;

type BoxFut<'a, T> = Pin<Box<dyn Future<Output = Result<T, String>> + Send + 'a>>;

/// Signature every wired resolver shares.
pub type ResolveFn =
    for<'a> fn(&'a ResolveContext<'a>, &'a MapSettings) -> BoxFut<'a, BlockPayload>;

fn uint(settings: &MapSettings, key: &str, default: u32) -> u32 {
    settings
        .get(key)
        .and_then(serde_json::Value::as_u64)
        .map_or(default, |v| v.clamp(1, 100) as u32)
}

fn bool_of(settings: &MapSettings, key: &str) -> bool {
    settings
        .get(key)
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

fn str_of<'a>(settings: &'a MapSettings, key: &str) -> Option<&'a str> {
    settings.get(key).and_then(serde_json::Value::as_str)
}

/// A single card's markup, for tests of the shared card.
#[must_use]
pub fn card_html_for_test(p: &PostCardData) -> String {
    let mut out = String::new();
    card_html(&mut out, p, true, true);
    out
}

/// One entry card, shared by every card-list block so they all read the
/// same to a stylesheet.
fn card_html(html: &mut String, p: &PostCardData, show_image: bool, show_excerpt: bool) {
    let _ = std::fmt::Write::write_fmt(
        html,
        format_args!(
            "<article class=\"vy-post-card\">\
             <a class=\"vy-post-card__link\" href=\"{}\">{}</a>\
             <span class=\"vy-post-card__meta\">{} · {}</span>",
            esc(&p.url),
            esc(&p.title),
            esc(&p.author),
            esc(&p.date),
        ),
    );
    if show_image {
        if let (Some(thumb), Some(hash)) = (&p.thumb_url, &p.blurhash) {
            // A cropped card shows the part the author pointed at.
            let focal = p
                .thumb_focal
                .as_ref()
                .filter(|f| {
                    f.chars()
                        .all(|c| c.is_ascii_digit() || matches!(c, '%' | ' ' | '.'))
                })
                .map_or(String::new(), |f| format!(" style=\"object-position:{f}\""));
            let _ = std::fmt::Write::write_fmt(
                html,
                format_args!(
                    "<img class=\"vy-post-card__thumb\" src=\"{}\" \
                     alt=\"\" loading=\"lazy\" data-blurhash=\"{}\"{focal}>",
                    esc(thumb),
                    esc(hash)
                ),
            );
        }
    }
    if show_excerpt && !p.excerpt.is_empty() {
        let _ = std::fmt::Write::write_fmt(
            html,
            format_args!("<p class=\"vy-post-card__excerpt\">{}</p>", esc(&p.excerpt)),
        );
    }
    html.push_str("</article>");
}

/// `latest-posts`: card list with thumbnails + blurhash LQIP hooks.
///
/// A special case of `collection` that predates it: same query, same
/// cards, fixed to published posts newest-first.
pub fn latest_posts<'a>(
    ctx: &'a ResolveContext<'a>,
    settings: &'a MapSettings,
) -> BoxFut<'a, BlockPayload> {
    let count = uint(settings, "count", 5);
    let category = str_of(settings, "category");
    let fut = ctx
        .queries
        .entries("post", EntrySort::Newest, count, category);
    Box::pin(async move {
        let posts = fut.await?;
        if posts.is_empty() {
            return Ok(BlockPayload::Empty);
        }
        let mut html = String::from("<div class=\"vy-latest-posts\">");
        for p in &posts {
            card_html(&mut html, p, true, true);
        }
        html.push_str("</div>");
        Ok(BlockPayload::Html(html))
    })
}

/// `collection`: entries of any content source, as cards.
///
/// The binding names the source; a source that no longer exists (its
/// plugin was disabled) yields an empty section, never an error.
pub fn collection<'a>(
    ctx: &'a ResolveContext<'a>,
    settings: &'a MapSettings,
) -> BoxFut<'a, BlockPayload> {
    // Settings were validated on the way in; a tree that predates the
    // validator still must not take the page down, so a broken binding
    // renders as nothing.
    let Ok(Some(bind)) = Binding::from_settings(settings) else {
        return Box::pin(std::future::ready(Ok(BlockPayload::Empty)));
    };
    let columns = uint(settings, "columns", 3).clamp(1, 4);
    let show_image = settings
        .get("show_image")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(true);
    let show_excerpt = settings
        .get("show_excerpt")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(true);
    let heading = str_of(settings, "heading").map(str::to_owned);
    Box::pin(async move {
        let posts = ctx.queries.bound_entries(&bind).await?;
        if posts.is_empty() {
            return Ok(BlockPayload::Empty);
        }
        let mut html = String::new();
        if let Some(h) = heading.filter(|h| !h.trim().is_empty()) {
            let _ = std::fmt::Write::write_fmt(
                &mut html,
                format_args!("<h2 class=\"vy-section-title\">{}</h2>", esc(&h)),
            );
        }
        let _ = std::fmt::Write::write_fmt(
            &mut html,
            format_args!("<div class=\"vy-collection\" style=\"--vy-cols:{columns}\">"),
        );
        for p in &posts {
            card_html(&mut html, p, show_image, show_excerpt);
        }
        html.push_str("</div>");
        Ok(BlockPayload::Html(html))
    })
}

/// `categories-list`.
pub fn categories_list<'a>(
    ctx: &'a ResolveContext<'a>,
    settings: &'a MapSettings,
) -> BoxFut<'a, BlockPayload> {
    let show_counts = bool_of(settings, "show_counts");
    let fut = ctx.queries.categories(show_counts);
    Box::pin(async move {
        let list = term_list(&fut.await?, "vy-categories");
        if list.is_empty() {
            return Ok(BlockPayload::Empty);
        }
        Ok(BlockPayload::Html(list))
    })
}

/// `tag-cloud`: usage-weighted links.
pub fn tag_cloud<'a>(
    ctx: &'a ResolveContext<'a>,
    settings: &'a MapSettings,
) -> BoxFut<'a, BlockPayload> {
    let count = uint(settings, "count", 20);
    let fut = ctx.queries.popular_tags(count);
    Box::pin(async move {
        let list = term_list(&fut.await?, "vy-tag-cloud");
        if list.is_empty() {
            return Ok(BlockPayload::Empty);
        }
        Ok(BlockPayload::Html(list))
    })
}

fn term_list(links: &[super::queries::TermLinkData], class: &str) -> String {
    if links.is_empty() {
        return String::new();
    }
    let max: i64 = links
        .iter()
        .filter_map(|l| l.count)
        .max()
        .unwrap_or(1)
        .max(1);
    let max_f = f64::from(i32::try_from(max).unwrap_or(i32::MAX));
    let mut out = format!("<ul class=\"{class}\">");
    for t in links {
        let weight = t.count.map_or(1.0_f64, |c| {
            let cf = f64::from(i32::try_from(c).unwrap_or(i32::MAX));
            1.0 + cf * 2.0 / max_f
        });
        let count_suffix = match t.count {
            Some(c) => format!(" <span class=\"vy-term-count\">({c})</span>"),
            None => String::new(),
        };
        let _ = std::fmt::Write::write_fmt(
            &mut out,
            format_args!(
                "<li style=\"--vy-weight:{weight:.2}\"><a href=\"{}\">{}</a>{count_suffix}</li>",
                esc(&t.url),
                esc(&t.name)
            ),
        );
    }
    out.push_str("</ul>");
    out
}

/// `archives`: monthly links.
pub fn archives<'a>(
    ctx: &'a ResolveContext<'a>,
    settings: &'a MapSettings,
) -> BoxFut<'a, BlockPayload> {
    let months = uint(settings, "months", 12);
    let fut = ctx.queries.monthly_archives(months);
    Box::pin(async move {
        let list = fut.await?;
        if list.is_empty() {
            return Ok(BlockPayload::Empty);
        }
        let mut out = String::from("<ul class=\"vy-archives\">");
        for m in &list {
            let _ = std::fmt::Write::write_fmt(
                &mut out,
                format_args!(
                    "<li><a href=\"{}\">{}</a> \
                     <span class=\"vy-term-count\">({})</span></li>",
                    esc(&m.url),
                    esc(&m.label),
                    m.count
                ),
            );
        }
        out.push_str("</ul>");
        Ok(BlockPayload::Html(out))
    })
}

/// `search-box`: plain GET form to /?s=.
/// A greeting that names the signed-in reader, filled in by the browser.
///
/// The only block that ships JavaScript, and the only one that can: every
/// other block renders into a page the cache then serves to everyone, so
/// anything reader-specific in one would show the first visitor's name to
/// the next. This renders a cacheable placeholder and fetches the name
/// from an uncached endpoint after load — the standard way to keep a page
/// shared and still say "welcome back".
///
/// The script is inline and tiny rather than a file, because it exists
/// only on pages whose layout actually places this block, and a request
/// for a few hundred bytes would cost more than the bytes.
pub fn viewer_greeting<'a>(
    _ctx: &'a ResolveContext<'a>,
    settings: &'a MapSettings,
) -> BoxFut<'a, BlockPayload> {
    let greeting = str_of(settings, "greeting")
        .unwrap_or("Welcome back")
        .to_owned();
    let anonymous = str_of(settings, "anonymous").unwrap_or("").to_owned();
    Box::pin(async move {
        Ok(BlockPayload::Html(format!(
            "<p class=\"vy-viewer\" data-vy-viewer hidden></p>             <p class=\"vy-viewer vy-viewer--anon\" data-vy-viewer-anon{}>{}</p>             <script>(function(){{             var s=document.querySelector('[data-vy-viewer]'),             a=document.querySelector('[data-vy-viewer-anon]');             fetch('/api/v1/viewer',{{credentials:'same-origin'}})             .then(function(r){{return r.ok?r.json():null}})             .then(function(v){{             if(!v||!v.signedIn)return;             s.textContent={}+' '+v.displayName;s.hidden=false;             if(a)a.hidden=true;}})             .catch(function(){{}});}})();</script>",
            if anonymous.is_empty() { " hidden" } else { "" },
            esc(&anonymous),
            serde_json::Value::String(greeting),
        )))
    })
}

pub fn search_box<'a>(
    _ctx: &'a ResolveContext<'a>,
    settings: &'a MapSettings,
) -> BoxFut<'a, BlockPayload> {
    let placeholder = str_of(settings, "placeholder")
        .unwrap_or("Search…")
        .to_owned();
    Box::pin(async move {
        Ok(BlockPayload::Html(format!(
            "<form class=\"vy-search\" action=\"/search\" method=\"get\" role=\"search\">\
             <input type=\"search\" name=\"s\" placeholder=\"{}\" \
             aria-label=\"Search\"><button type=\"submit\">Go</button></form>",
            esc(&placeholder)
        )))
    })
}

/// `breadcrumbs`: static trail from context (engine fills ancestors later).
pub fn breadcrumbs<'a>(
    _ctx: &'a ResolveContext<'a>,
    _settings: &'a MapSettings,
) -> BoxFut<'a, BlockPayload> {
    Box::pin(async move {
        Ok(BlockPayload::Html(
            "<nav class=\"vy-breadcrumbs\" aria-label=\"Breadcrumb\"><ol>\
             <li><a href=\"/\">Home</a></li></ol></nav>"
                .to_owned(),
        ))
    })
}

/// `comments`: the approved thread for the current post, plus the form
/// that lets a visitor add to it.
///
/// The form is plain HTML posting to `/comment`: no JavaScript, so it
/// works under the site's `script-src 'self'` policy and with scripting
/// switched off. A reply is a link back to the same page carrying
/// `?reply_to=`, which re-renders the form with a parent set.
pub fn comments<'a>(
    ctx: &'a ResolveContext<'a>,
    settings: &'a MapSettings,
) -> BoxFut<'a, BlockPayload> {
    let per_page = uint(settings, "per_page", 20);
    let Some(post_id) = ctx.post_id else {
        return Box::pin(async { Ok(BlockPayload::Empty) });
    };
    let reply_to = ctx.reply_to;
    let fut = ctx.queries.approved_comments(post_id);
    Box::pin(async move {
        let tree = fut.await?;
        let count = count_comments(&tree);
        let title = match count {
            0 => "Comments".to_owned(),
            1 => "1 comment".to_owned(),
            n => format!("{n} comments"),
        };
        let body = if tree.is_empty() {
            String::from("<p class=\"vy-comments__empty\">Be the first to comment.</p>")
        } else {
            render_comments(&tree, per_page, 1)
        };
        Ok(BlockPayload::Html(format!(
            "<section class=\"vy-comments\" id=\"comments\">\
             <h2 class=\"vy-comments__title\">{title}</h2>{body}{}</section>",
            comment_form(post_id, reply_to)
        )))
    })
}

fn count_comments(nodes: &[CommentNodeData]) -> usize {
    nodes.iter().map(|n| 1 + count_comments(&n.children)).sum()
}

/// The comment form. `reply_to` nests the new comment under an existing one.
fn comment_form(post_id: i64, reply_to: Option<i64>) -> String {
    let replying = reply_to.map_or_else(String::new, |id| {
        format!(
            "<p class=\"vy-comment-form__replying\">Replying to \
             <a href=\"#comment-{id}\">a comment</a> · \
             <a href=\"?#respond\" rel=\"nofollow\">write a new one instead</a></p>\
             <input type=\"hidden\" name=\"parent_id\" value=\"{id}\">"
        )
    });
    format!(
        "<form class=\"vy-comment-form\" id=\"respond\" method=\"post\" action=\"/comment\">\
         <h3 class=\"vy-comment-form__title\">Leave a comment</h3>{replying}\
         <input type=\"hidden\" name=\"post_id\" value=\"{post_id}\">\
         <p class=\"vy-field\"><label for=\"vy-c-name\">Name</label>\
         <input id=\"vy-c-name\" name=\"author_name\" type=\"text\" required maxlength=\"80\" \
         autocomplete=\"name\"></p>\
         <p class=\"vy-field\"><label for=\"vy-c-email\">Email</label>\
         <input id=\"vy-c-email\" name=\"author_email\" type=\"email\" required maxlength=\"120\" \
         autocomplete=\"email\">\
         <span class=\"vy-field__hint\">Not published.</span></p>\
         <p class=\"vy-field\"><label for=\"vy-c-body\">Comment</label>\
         <textarea id=\"vy-c-body\" name=\"content\" rows=\"5\" required maxlength=\"5000\">\
         </textarea></p>\
         <p class=\"vy-field vy-field--trap\" aria-hidden=\"true\">\
         <label for=\"vy-c-url\">Leave this empty</label>\
         <input id=\"vy-c-url\" name=\"website\" type=\"text\" tabindex=\"-1\" autocomplete=\"off\">\
         </p>\
         <p><button type=\"submit\">Post comment</button></p></form>"
    )
}

fn render_comments(nodes: &[CommentNodeData], per_page: u32, depth: usize) -> String {
    if depth > 4 || nodes.is_empty() {
        return String::new();
    }
    // The enclosing <section> belongs to `comments()`; this renders the
    // list only, at any depth.
    let mut out = if depth == 1 {
        String::from("<ol class=\"vy-comment-list\">")
    } else {
        String::from("<ol class=\"vy-comment-children\">")
    };
    for node in nodes.iter().take(per_page as usize) {
        let _ = std::fmt::Write::write_fmt(
            &mut out,
            format_args!(
                "<li class=\"vy-comment\" id=\"comment-{}\">\
                 <span class=\"vy-comment__meta\">{} · {}</span>\
                 <div class=\"vy-comment__body\">{}</div>\
                 <p class=\"vy-comment__actions\">\
                 <a href=\"?reply_to={}#respond\" rel=\"nofollow\">Reply</a></p>{}</li>",
                node.id,
                esc(&node.author),
                esc(&node.date),
                node.html,
                node.id,
                render_comments(&node.children, per_page, depth + 1)
            ),
        );
    }
    out.push_str("</ol>");
    out
}

/// `menu`: renders a named navigation menu.
pub fn navigation_menu<'a>(
    ctx: &'a ResolveContext<'a>,
    settings: &'a MapSettings,
) -> BoxFut<'a, BlockPayload> {
    let slug: &str = str_of(settings, "slug").unwrap_or("main");
    let fut = ctx.queries.navigation_menu(slug);
    Box::pin(async move {
        let html = fut.await?;
        if html.is_empty() {
            return Ok(BlockPayload::Empty);
        }
        // The drawer needs no JavaScript: on phones the label is the menu
        // button and the checkbox inside it does the folding; on wider
        // screens the CSS hides the label and the list simply shows. A
        // closed <details> would have been simpler, but a browser never
        // lays out the children of a closed details no matter what the
        // stylesheet says, so the desktop menu was invisible. The label
        // wraps its checkbox rather than pointing at an id, so two menus
        // on one page (header and footer, say) never share one.
        Ok(BlockPayload::Html(format!(
            "<nav class=\"vy-nav\"><label class=\"vy-nav-toggle\" aria-label=\"Menu\">\
             <input type=\"checkbox\" class=\"vy-nav-check\"></label>{html}</nav>"
        )))
    })
}

/// `docs-nav`: page-tree navigation for documentation sites.
pub fn docs_nav<'a>(
    ctx: &'a ResolveContext<'a>,
    _settings: &'a MapSettings,
) -> BoxFut<'a, BlockPayload> {
    let fut = ctx.queries.page_tree();
    Box::pin(async move {
        let nodes = fut.await?;
        if nodes.is_empty() {
            return Ok(BlockPayload::Empty);
        }
        let mut out = String::from("<nav class=\"vy-docs-nav\"><ul class=\"vy-menu\">");
        for n in &nodes {
            let indent = "  ".repeat(n.depth.saturating_sub(1) as usize);
            let _ = std::fmt::Write::write_fmt(
                &mut out,
                format_args!(
                    "{indent}<li class=\"vy-docs-nav__d{}\"><a href=\"{}\">{}</a></li>",
                    n.depth,
                    esc(&n.url),
                    esc(&n.title)
                ),
            );
        }
        out.push_str("</ul></nav>");
        Ok(BlockPayload::Html(out))
    })
}

/// `toc`: table of contents from the current page's headings.
pub fn toc<'a>(
    ctx: &'a ResolveContext<'a>,
    _settings: &'a MapSettings,
) -> BoxFut<'a, BlockPayload> {
    let entries = ctx.content_blocks.map(crate::renderer::toc_from_blocks);
    Box::pin(async move {
        let Some(entries) = entries else {
            return Ok(BlockPayload::Empty);
        };
        if entries.is_empty() {
            return Ok(BlockPayload::Empty);
        }
        let mut out = String::from("<nav class=\"vy-toc\" aria-label=\"Table of contents\"><ol>");
        for e in &entries {
            let _ = std::fmt::Write::write_fmt(
                &mut out,
                format_args!(
                    "<li class=\"vy-toc__l{}\"><a href=\"#{}\">{}</a></li>",
                    e.level,
                    esc(e.anchor.trim_start_matches('#')),
                    esc(&e.text)
                ),
            );
        }
        out.push_str("</ol></nav>");
        Ok(BlockPayload::Html(out))
    })
}

/// `related-posts`: the posts nearest in meaning to the one being read.
/// Renders nothing outside a single entry or when no embedding exists.
pub fn related_posts<'a>(
    ctx: &'a ResolveContext<'a>,
    settings: &'a MapSettings,
) -> BoxFut<'a, BlockPayload> {
    let count = uint(settings, "count", 5).min(10);
    let heading = str_of(settings, "heading").unwrap_or("Related").to_owned();
    Box::pin(async move {
        let Some(post_id) = ctx.post_id else {
            return Ok(BlockPayload::Empty);
        };
        let posts = ctx.queries.related_posts(post_id, count).await?;
        if posts.is_empty() {
            return Ok(BlockPayload::Empty);
        }
        let mut html = format!(
            "<section class=\"vy-related-posts\"><h2 class=\"vy-related-posts__heading\">{}</h2><ul>",
            esc(&heading)
        );
        for p in &posts {
            let _ = std::fmt::Write::write_fmt(
                &mut html,
                format_args!(
                    "<li class=\"vy-related-posts__item\"><a href=\"{}\">{}</a>{}</li>",
                    esc(&p.url),
                    esc(&p.title),
                    if p.excerpt.is_empty() {
                        String::new()
                    } else {
                        format!(
                            "<p class=\"vy-related-posts__excerpt\">{}</p>",
                            esc(&p.excerpt)
                        )
                    }
                ),
            );
        }
        html.push_str("</ul></section>");
        Ok(BlockPayload::Html(html))
    })
}

/// `read-aloud`: an audio player for the post's generated recording.
pub fn read_aloud<'a>(
    ctx: &'a ResolveContext<'a>,
    settings: &'a MapSettings,
) -> BoxFut<'a, BlockPayload> {
    let label = str_of(settings, "label")
        .unwrap_or("Listen to this post")
        .to_owned();
    Box::pin(async move {
        let Some(post_id) = ctx.post_id else {
            return Ok(BlockPayload::Empty);
        };
        let Some(url) = ctx.queries.post_audio_url(post_id).await? else {
            return Ok(BlockPayload::Empty);
        };
        Ok(BlockPayload::Html(format!(
            "<figure class=\"vy-read-aloud\"><figcaption>{}</figcaption>\
             <audio controls preload=\"none\" src=\"{}\"></audio></figure>",
            esc(&label),
            esc(&url)
        )))
    })
}

/// Renders nothing: containers contribute structure, not content.
pub fn empty<'a>(
    _ctx: &'a ResolveContext<'a>,
    _settings: &'a MapSettings,
) -> BoxFut<'a, BlockPayload> {
    Box::pin(std::future::ready(Ok(BlockPayload::Empty)))
}
