//! Index round-trip + query tests with a temp dir.

#![allow(clippy::pedantic, clippy::unwrap_used)]

use vyasa_search::{escape_query, IndexManager, SearchDoc};

fn tmpdir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "vy-search-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn doc(id: u64, title: &str, body: &str) -> SearchDoc {
    SearchDoc {
        id,
        post_type: String::from("post"),
        slug: format!("post-{id}"),
        title: title.to_owned(),
        body: body.to_owned(),
    }
}

#[test]
fn round_trip_and_query() {
    let dir = tmpdir();
    let mgr = IndexManager::open(&dir).unwrap();
    mgr.upsert(&doc(
        1,
        "Vyasa intro",
        "An introduction to Vyasa publishing",
    ))
    .unwrap();
    mgr.upsert(&doc(2, "Cooking pasta", "Boil water, add salt and pasta"))
        .unwrap();

    let hits = mgr.search("introduction", None, 10, 0).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].id, 1);
    let hits = mgr.search("pasta", None, 10, 0).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].slug, "post-2");
}

#[test]
fn trash_deletes_from_index() {
    let dir = tmpdir();
    let mgr = IndexManager::open(&dir).unwrap();
    mgr.upsert(&doc(5, "To be deleted", "unique deletion marker text"))
        .unwrap();
    mgr.delete(5).unwrap();
    let hits = mgr.search("deletion marker", None, 10, 0).unwrap();
    assert!(
        hits.is_empty(),
        "deleted doc must not match: {:?}",
        hits.iter().map(|h| h.id).collect::<Vec<_>>()
    );
}

#[test]
fn hostile_queries_do_not_crash() {
    let dir = tmpdir();
    let mgr = IndexManager::open(&dir).unwrap();
    for q in [
        "\"",
        "a:b AND (c OR d)",
        "\\",
        "title:^foo~2",
        "{}[]()!+ -",
        "",
        "日本語テキスト",
    ] {
        let _ = mgr.search(q, None, 10, 0);
    }
}

#[test]
fn escape_neutralizes_syntax() {
    assert_eq!(escape_query("a+b"), "a b");
}

#[test]
fn snippets_highlight_the_matched_term() {
    // This is the test that was missing. The body field was indexed but not
    // stored, and tantivy builds a snippet from the *stored* text, so every
    // hit came back with an empty snippet — invisibly, because nothing
    // asserted on it.
    let dir = tmpdir();
    let mgr = IndexManager::open(&dir).unwrap();
    mgr.upsert(&doc(
        1,
        "Deployment notes",
        "The render cache is purged by domain events whenever a post changes.",
    ))
    .unwrap();

    let hits = mgr.search("purged", None, 10, 0).unwrap();
    assert_eq!(hits.len(), 1);
    assert!(
        hits[0].snippet.contains("<mark>"),
        "expected a highlight, got {:?}",
        hits[0].snippet
    );
    assert!(
        hits[0].snippet.to_lowercase().contains("purged"),
        "the highlight must cover the term searched for, got {:?}",
        hits[0].snippet
    );
}

#[test]
fn stored_fields_survive_the_round_trip() {
    // Title is stored for the same reason as body: a caller that does not
    // re-read the row from the database gets it from here.
    let dir = tmpdir();
    let mgr = IndexManager::open(&dir).unwrap();
    mgr.upsert(&doc(7, "A Distinctive Title", "body text"))
        .unwrap();

    let hits = mgr.search("Distinctive", None, 10, 0).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].title, "A Distinctive Title");
    assert_eq!(hits[0].slug, "post-7");
    assert_eq!(hits[0].id, 7);
}

#[test]
fn an_index_written_with_an_older_schema_is_rebuilt_not_served_empty() {
    use tantivy::schema::{Schema, STORED, STRING};

    let dir = tmpdir();
    // Stand in for a pre-existing index: a different schema in the same
    // directory. Opening it must not yield a manager that answers with
    // empty fields forever.
    let mut old = Schema::builder();
    old.add_text_field("something_else", STORED | STRING);
    tantivy::Index::create_in_dir(&dir, old.build()).unwrap();

    let mgr = IndexManager::open(&dir).expect("must rebuild rather than fail");
    mgr.upsert(&doc(1, "Fresh start", "indexed after the rebuild"))
        .unwrap();
    let hits = mgr.search("rebuild", None, 10, 0).unwrap();
    assert_eq!(hits.len(), 1);
    assert!(hits[0].snippet.contains("<mark>"));
}

#[test]
fn a_literal_bold_tag_in_a_post_is_not_mistaken_for_a_highlight() {
    // The generator escapes source text before inserting its own markup, so
    // an author writing "<b>" must not come back as a highlight.
    let dir = tmpdir();
    let mgr = IndexManager::open(&dir).unwrap();
    mgr.upsert(&doc(
        1,
        "Markup",
        "Use <b>bold</b> sparingly when writing documentation.",
    ))
    .unwrap();

    let hits = mgr.search("sparingly", None, 10, 0).unwrap();
    assert_eq!(hits.len(), 1);
    let snippet = &hits[0].snippet;
    assert!(
        snippet.contains("&lt;b&gt;"),
        "author markup stays escaped: {snippet:?}"
    );
    // The only <mark> present is the one wrapping the searched term.
    assert_eq!(snippet.matches("<mark>").count(), 1, "{snippet:?}");
}
