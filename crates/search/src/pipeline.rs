//! Event subscriber: keeps the index in sync with post lifecycle events.

use vyasa_core::events::Event;

use crate::index::{IndexManager, SearchDoc};

/// Handles one event against the index.
///
/// # Errors
/// Returns index write failures to the caller (the serve loop logs them).
pub fn handle_event(
    manager: &IndexManager,
    event: Event,
    doc_lookup: &dyn Fn(i64) -> Option<SearchDoc>,
) -> Result<(), vyasa_common::AppError> {
    match event {
        // A post that left public view must leave the index, otherwise it
        // keeps surfacing in search after being trashed.
        Event::Trashed(p) => manager.delete(u64::try_from(p.post_id).unwrap_or_default()),
        Event::Deleted(p) => manager.delete(u64::try_from(p.post_id).unwrap_or_default()),
        Event::Published(p) => upsert(manager, p.post_id, doc_lookup),
        Event::Updated(p) => upsert(manager, p.post_id, doc_lookup),
        Event::Restored(p) => upsert(manager, p.post_id, doc_lookup),
        _ => Ok(()),
    }
}

/// Indexes a post, or removes it when the lookup declines to supply a
/// document — which is how a post that stopped being publishable (drafted,
/// scheduled, made private) drops out of search.
fn upsert(
    manager: &IndexManager,
    post_id: i64,
    doc_lookup: &dyn Fn(i64) -> Option<SearchDoc>,
) -> Result<(), vyasa_common::AppError> {
    match doc_lookup(post_id) {
        Some(doc) => manager.upsert(&doc),
        None => manager.delete(u64::try_from(post_id).unwrap_or_default()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::IndexManager;

    fn doc(id: u64) -> SearchDoc {
        SearchDoc {
            id,
            post_type: "post".into(),
            slug: format!("p{id}"),
            title: format!("Title {id}"),
            body: "searchable body text".into(),
        }
    }

    fn manager() -> (IndexManager, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let m = IndexManager::open(dir.path()).expect("open index");
        (m, dir)
    }

    #[test]
    fn publishing_indexes_and_trashing_removes() {
        let (m, _dir) = manager();
        let lookup = |id: i64| Some(doc(u64::try_from(id).unwrap_or_default()));

        handle_event(
            &m,
            Event::Published(vyasa_core::events::PostPublished {
                post_id: 7,
                author_id: 1,
            }),
            &lookup,
        )
        .expect("index");
        assert_eq!(m.search("searchable", None, 10, 0).expect("query").len(), 1);

        handle_event(
            &m,
            Event::Trashed(vyasa_core::events::PostTrashed { post_id: 7 }),
            &lookup,
        )
        .expect("delete");
        assert_eq!(
            m.search("searchable", None, 10, 0).expect("query").len(),
            0,
            "a trashed post must not stay searchable"
        );
    }

    #[test]
    fn an_update_that_hides_a_post_removes_it() {
        let (m, _dir) = manager();
        let present = |id: i64| Some(doc(u64::try_from(id).unwrap_or_default()));
        handle_event(
            &m,
            Event::Published(vyasa_core::events::PostPublished {
                post_id: 3,
                author_id: 1,
            }),
            &present,
        )
        .expect("index");

        // The lookup declines: the post is no longer publishable.
        let gone = |_: i64| None;
        handle_event(
            &m,
            Event::Updated(vyasa_core::events::PostUpdated { post_id: 3 }),
            &gone,
        )
        .expect("remove");
        assert_eq!(m.search("searchable", None, 10, 0).expect("query").len(), 0);
    }
}
