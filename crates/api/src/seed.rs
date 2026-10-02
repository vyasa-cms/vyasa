//! Development data generator (`vyasa dev seed`).
//!
//! Phase 46 needs a dataset big enough for archive pagination and query
//! plans to behave like a real site rather than a fixture; the same data
//! doubles as demo content for screenshots.
//!
//! Deterministic by construction: no randomness, so two runs at the same
//! size produce the same corpus and a benchmark comparison is meaningful.

use vyasa_core::block::doc::BlockDocument;
use vyasa_core::block::types::{Block, BlockKind};
use vyasa_core::post::service::CreatePost;
use vyasa_db::content_models::{PostStatus, PostType};

use crate::state::AppState;

/// Word pool for body text. Real-ish word lengths matter: the search index
/// and the excerpt logic both behave differently on `lorem ipsum` than on
/// single-letter filler.
const WORDS: [&str; 24] = [
    "render",
    "cache",
    "theme",
    "block",
    "editor",
    "publish",
    "archive",
    "taxonomy",
    "comment",
    "webhook",
    "plugin",
    "search",
    "index",
    "queue",
    "migration",
    "session",
    "capability",
    "sandbox",
    "template",
    "token",
    "revision",
    "schedule",
    "media",
    "digest",
];

/// Builds a deterministic sentence from `seed`.
fn sentence(seed: usize, words: usize) -> String {
    let mut out = String::new();
    for i in 0..words {
        if i > 0 {
            out.push(' ');
        }
        out.push_str(WORDS[(seed.wrapping_mul(7).wrapping_add(i * 13)) % WORDS.len()]);
    }
    out.push('.');
    out
}

/// Builds a body of `paragraphs` paragraphs.
fn body(seed: usize, paragraphs: usize) -> BlockDocument {
    let blocks = (0..paragraphs)
        .map(|p| Block {
            kind: BlockKind::Paragraph,
            plugin_kind: None,
            attrs: serde_json::json!({ "text": sentence(seed + p, 30) }),
            children: Vec::new(),
        })
        .collect();
    BlockDocument::new(blocks)
}

/// Creates `count` published posts authored by `author_id`.
///
/// Existing content is left alone: seeding a database that already has
/// posts adds to it rather than replacing it, because losing real content
/// to a development command is not a recoverable mistake.
///
/// # Errors
/// Propagates post-service failures.
pub async fn run(
    state: &AppState,
    count: u32,
    author_id: i64,
) -> Result<u32, vyasa_common::AppError> {
    let mut made = 0u32;
    for i in 0..count {
        let n = i as usize;
        let input = CreatePost {
            post_type: PostType::Post,
            status: PostStatus::Published,
            title: format!(
                "Seed post {}: {}",
                i + 1,
                sentence(n, 4).trim_end_matches('.')
            ),
            // Explicit slug keeps the corpus stable; the service still
            // de-duplicates if the database already holds one.
            slug: Some(format!("seed-post-{}", i + 1)),
            content: body(n, 3),
            excerpt: Some(sentence(n + 100, 18)),
            author_id,
            parent_id: None,
            scheduled_for: None,
            password: None,
            term_ids: None,
            layout: None,
        };
        match state.posts.create(input).await {
            Ok(_) => made += 1,
            Err(err) => {
                tracing::warn!("seed post {i} failed: {err}");
            }
        }
    }
    Ok(made)
}

#[cfg(test)]
mod tests {
    use super::{body, sentence, WORDS};

    #[test]
    fn generation_is_deterministic() {
        // A benchmark comparing two runs is only meaningful if the corpus
        // is identical.
        assert_eq!(sentence(3, 10), sentence(3, 10));
        assert_ne!(sentence(3, 10), sentence(4, 10));
    }

    #[test]
    fn sentences_use_real_words_and_end_cleanly() {
        let s = sentence(1, 5);
        assert!(s.ends_with('.'));
        assert_eq!(s.trim_end_matches('.').split(' ').count(), 5);
        for word in s.trim_end_matches('.').split(' ') {
            assert!(WORDS.contains(&word), "{word} is not from the pool");
        }
    }

    #[test]
    fn bodies_validate_against_the_block_schema() {
        // Seeded content must be indistinguishable from content the editor
        // would produce, or benchmarks measure the wrong thing.
        let doc = body(0, 3);
        assert_eq!(doc.blocks.len(), 3);
        doc.validate().expect("seeded blocks must be valid");
    }
}
