//! Keeps the Tantivy index in step with the post lifecycle.
//!
//! The search crate has always had an index, a query pipeline and an event
//! handler; nothing ever ran them. `/search` fell back to a SQL `LIKE`, and
//! the index stayed empty however many posts were published.
//!
//! This module supplies the missing piece: a task that subscribes to the
//! domain event bus and applies each post event to the index, plus a
//! one-shot rebuild for populating it from the database.
//!
//! Tantivy allows one writer, and the index lives on local disk, so each
//! instance keeps its **own** copy — a local materialised view of the
//! posts table rather than shared state. That is what makes it work across
//! instances: the event bridge relays a publish to every instance, and
//! each applies it to its own index. What a second instance cannot get
//! from the bus is everything that happened before it booted, which is
//! what [`converge_on_boot`] is for.

use std::sync::Arc;

use vyasa_core::events::{self, Event};
use vyasa_db::content_models::{PostRow, PostStatus};
use vyasa_db::repo::posts::PostFilter;
use vyasa_search::index::{IndexManager, SearchDoc};

use crate::state::AppState;

/// Only published posts belong in the index — search must not leak drafts,
/// scheduled posts or private ones.
fn is_publicly_searchable(row: &PostRow) -> bool {
    row.status == PostStatus::Published && row.password_hash.is_none()
}

/// Whether entries of this type belong in public search at all.
///
/// Reusable patterns have no public URL, and a plugin type that is not
/// currently served publicly would put results in front of visitors that
/// lead nowhere (or worse, surface a type its plugin declared private).
/// `served_custom` is the live set of publicly-served plugin type slugs.
fn type_is_searchable(
    post_type: vyasa_db::content_models::PostType,
    served_custom: &std::collections::HashSet<String>,
) -> bool {
    use vyasa_db::content_models::PostType;
    match post_type {
        PostType::Post | PostType::Page => true,
        PostType::Block => false,
        PostType::Custom(slug) => served_custom.contains(slug),
    }
}

/// Builds the indexable document for a post, or `None` when the post should
/// not be searchable. Returning `None` makes the pipeline delete it, so a
/// post that is unpublished drops out of results.
///
/// `fields` are the entry's non-empty text field values: indexed with the
/// body, so an entry is found by what its fields say. The index schema
/// does not change when fields do — values ride in the body text.
fn doc_for(
    row: &PostRow,
    served_custom: &std::collections::HashSet<String>,
    fields: &[(String, String)],
) -> Option<SearchDoc> {
    if !is_publicly_searchable(row) || !type_is_searchable(row.post_type, served_custom) {
        return None;
    }
    // A post whose body will not parse still deserves to be findable by
    // title, so fall back to an empty block list rather than skipping it.
    let blocks = vyasa_core::block::doc::BlockDocument::from_json(row.content.clone())
        .map(|d| d.blocks)
        .unwrap_or_default();
    Some(SearchDoc {
        id: u64::try_from(row.id).unwrap_or_default(),
        post_type: row.post_type.as_str().to_owned(),
        slug: row.slug.clone(),
        title: row.title.clone(),
        // Reuses the AI crate's extractor so indexed text matches what a
        // reader actually sees, rather than raw block JSON.
        body: {
            let mut body = vyasa_ai::assist::extract_text(&blocks, 100_000);
            for (_, value) in fields {
                body.push('\n');
                body.push_str(value);
            }
            body
        },
    })
}

/// The custom post types (plugins' and administrators') currently served
/// publicly.
async fn served_custom_types(state: &AppState) -> std::collections::HashSet<String> {
    crate::policy::PublicTypes::load(&state.plugin_surface, &state.pool)
        .await
        .into_set()
}

/// Rows fetched per batch when rebuilding.
const REINDEX_PAGE: u32 = 200;

/// Rebuilds the whole index from the database.
///
/// # Errors
/// Propagates repository and index-write failures.
pub async fn reindex_all(state: &AppState) -> Result<usize, vyasa_common::AppError> {
    let Some(manager) = state.search_index.clone() else {
        return Err(vyasa_common::AppError::internal_msg(
            "search index unavailable. Tantivy allows a single writer, so this \
             fails while the server holds the index: stop it first, or use \
             POST /api/v1/search/reindex to rebuild in-process.",
        ));
    };

    let served = served_custom_types(state).await;
    let mut defs = crate::entry_fields::Definitions::default();
    let mut indexed = 0usize;
    let mut offset = 0u32;
    loop {
        let filter = PostFilter {
            limit: REINDEX_PAGE,
            offset,
            ..PostFilter::default()
        };
        let rows = state.posts.list(&filter).await?;
        if rows.is_empty() {
            break;
        }
        let count = u32::try_from(rows.len()).unwrap_or(REINDEX_PAGE);
        for row in &rows {
            let fields = crate::entry_fields::text_values(
                defs.of(&state.pool, row.post_type).await,
                &row.meta,
            );
            match doc_for(row, &served, &fields) {
                Some(doc) => {
                    manager.upsert(&doc)?;
                    indexed += 1;
                }
                // Sweep out anything previously indexed that no longer
                // qualifies, so a rebuild also acts as a repair.
                None => manager.delete(u64::try_from(row.id).unwrap_or_default())?,
            }
        }
        if count < REINDEX_PAGE {
            break;
        }
        offset += count;
    }
    Ok(indexed)
}

/// Brings one type's entries in the index up to date: after the type
/// became public or non-public, or one of its fields was deleted or
/// changed kind (so a deleted field's text stops matching). Entries that
/// no longer qualify are removed. A server without an index does nothing.
///
/// # Errors
/// Propagates repository and index-write failures.
pub async fn reindex_type(
    state: &AppState,
    post_type: vyasa_db::content_models::PostType,
) -> Result<usize, vyasa_common::AppError> {
    let Some(manager) = state.search_index.clone() else {
        return Ok(0);
    };
    let served = served_custom_types(state).await;
    let mut defs = crate::entry_fields::Definitions::default();
    let mut indexed = 0usize;
    let mut offset = 0u32;
    loop {
        let filter = PostFilter {
            post_type: Some(post_type),
            limit: REINDEX_PAGE,
            offset,
            ..PostFilter::default()
        };
        let rows = state.posts.list(&filter).await?;
        if rows.is_empty() {
            break;
        }
        let count = u32::try_from(rows.len()).unwrap_or(REINDEX_PAGE);
        let mut docs = Vec::with_capacity(rows.len());
        for row in &rows {
            let fields = crate::entry_fields::text_values(
                defs.of(&state.pool, row.post_type).await,
                &row.meta,
            );
            docs.push((
                u64::try_from(row.id).unwrap_or_default(),
                doc_for(row, &served, &fields),
            ));
        }
        let manager = Arc::clone(&manager);
        indexed += tokio::task::spawn_blocking(move || {
            let mut n = 0usize;
            for (id, doc) in docs {
                match doc {
                    Some(doc) => {
                        manager.upsert(&doc)?;
                        n += 1;
                    }
                    None => manager.delete(id)?,
                }
            }
            Ok::<usize, vyasa_common::AppError>(n)
        })
        .await
        .map_err(|e| vyasa_common::AppError::internal_msg(format!("index task: {e}")))??;
        if count < REINDEX_PAGE {
            break;
        }
        offset += count;
    }
    Ok(indexed)
}

/// Subscribes to post events and keeps the index current for the life of the
/// process.
///
/// Runs on a blocking-friendly path: index writes take a lock and commit, so
/// they are done inside `spawn_blocking` to keep them off the async runtime's
/// worker threads.
pub fn spawn(state: &AppState) {
    let Some(manager) = state.search_index.clone() else {
        tracing::warn!("search index unavailable; search falls back to SQL matching");
        return;
    };
    let posts = state.posts.clone();
    let pool = state.pool.clone();
    let surface = std::sync::Arc::clone(&state.plugin_surface);
    let mut rx = events::subscribe();

    tokio::spawn(async move {
        loop {
            let event = match rx.recv().await {
                Ok(event) => event,
                // Lagged means the bus dropped events under load; the index
                // is now potentially stale, so say so rather than pretend.
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!("search indexer lagged {n} events; run `vyasa search reindex`");
                    continue;
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            };

            if let Err(err) = apply(&manager, &posts, &pool, &surface, event).await {
                // A failed index write must never take the server down.
                tracing::warn!("search index update failed: {err}");
            }
        }
    });
}

async fn apply(
    manager: &Arc<IndexManager>,
    posts: &vyasa_core::post::PostService,
    pool: &sqlx::PgPool,
    surface: &crate::plugin_surface::PluginSurface,
    event: Event,
) -> Result<(), vyasa_common::AppError> {
    // Resolve the row first: the pipeline's lookup is synchronous, but
    // reading the post is not.
    let fetched = match post_id_of(&event) {
        Some(id) => posts.get(id).await.ok(),
        None => None,
    };
    let served = crate::policy::PublicTypes::load(surface, pool)
        .await
        .into_set();
    let doc = match fetched.as_ref() {
        Some(row) => {
            let mut defs = crate::entry_fields::Definitions::default();
            let fields =
                crate::entry_fields::text_values(defs.of(pool, row.post_type).await, &row.meta);
            doc_for(row, &served, &fields)
        }
        None => None,
    };

    let manager = Arc::clone(manager);
    tokio::task::spawn_blocking(move || {
        vyasa_search::pipeline::handle_event(&manager, event, &|_| doc.clone())
    })
    .await
    .map_err(|e| vyasa_common::AppError::internal_msg(format!("index task: {e}")))?
}

const fn post_id_of(event: &Event) -> Option<i64> {
    match event {
        Event::Published(p) => Some(p.post_id),
        Event::Updated(p) => Some(p.post_id),
        Event::Restored(p) => Some(p.post_id),
        Event::Trashed(p) => Some(p.post_id),
        Event::Deleted(p) => Some(p.post_id),
        _ => None,
    }
}

/// Rebuilds the index at boot when it does not match the database.
///
/// A freshly started instance has an empty index — or, after downtime, one
/// missing everything published while it was away. Neither is visible from
/// the event bus, which only carries what happens next, so a new server
/// behind a load balancer would answer searches with nothing and no error.
///
/// Compares counts rather than diffing: the check runs once, an exact
/// answer would cost a full scan, and the only decision it drives is
/// whether to rebuild.
///
/// Runs in the background — a large index must not hold up serving, and a
/// site with a stale index still answers every other request.
pub fn converge_on_boot(state: &AppState) {
    let Some(manager) = state.search_index.clone() else {
        return;
    };
    let state = state.clone();
    tokio::spawn(async move {
        let filter = PostFilter {
            status: Some(PostStatus::Published),
            limit: 1,
            ..PostFilter::default()
        };
        let published = match state.posts.count(&filter).await {
            Ok(n) => u64::try_from(n).unwrap_or(0),
            Err(e) => {
                tracing::warn!("could not check the search index: {e}");
                return;
            }
        };
        let indexed = manager.doc_count();
        if indexed == published {
            tracing::debug!(indexed, "search index is in step");
            return;
        }
        tracing::info!(
            indexed,
            published,
            "search index is out of step; rebuilding it"
        );
        match reindex_all(&state).await {
            Ok(n) => tracing::info!("search index rebuilt with {n} posts"),
            Err(e) => tracing::warn!("search index rebuild failed: {e}"),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_publicly_served_types_are_searchable() {
        use vyasa_db::content_models::PostType;
        let ticket = PostType::register("support-ticket").expect("registers");
        let mut served = std::collections::HashSet::new();

        // Core content is always searchable; patterns never are.
        assert!(type_is_searchable(PostType::Post, &served));
        assert!(type_is_searchable(PostType::Page, &served));
        assert!(!type_is_searchable(PostType::Block, &served));

        // A plugin type joins search only while its plugin serves it
        // publicly — otherwise results would point at 404s, or leak a
        // type declared private.
        assert!(!type_is_searchable(ticket, &served));
        served.insert("support-ticket".to_owned());
        assert!(type_is_searchable(ticket, &served));
    }
}
