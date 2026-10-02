//! Query parsing, execution, and snippet generation.

use tantivy::collector::TopDocs;
use tantivy::query::{QueryParser, TermQuery};
use tantivy::schema::{IndexRecordOption, Term};
use tantivy::snippet::SnippetGenerator;
use tantivy::Searcher;
use vyasa_common::AppError;

use super::index::IndexManager;

fn owned_u64(v: Option<&tantivy::schema::OwnedValue>) -> u64 {
    match v {
        Some(tantivy::schema::OwnedValue::U64(n)) => *n,
        _ => 0,
    }
}

fn owned_text(v: Option<&tantivy::schema::OwnedValue>) -> String {
    match v {
        Some(tantivy::schema::OwnedValue::Str(s)) => s.clone(),
        _ => String::new(),
    }
}

/// One search hit.
#[derive(Debug, Clone)]
pub struct SearchHit {
    /// Post id.
    pub id: u64,
    /// Slug.
    pub slug: String,
    /// Title.
    pub title: String,
    /// Highlighted body snippet with `<mark>` around matches.
    pub snippet: String,
}

/// Rewrites tantivy's highlight markup to `<mark>`.
///
/// Tantivy emits `<b>`, which is presentational; `<mark>` is the element
/// that means "relevant in the current context", which is exactly what a
/// search highlight is, and it is what this crate's callers and the theme
/// stylesheet expect.
///
/// Safe as a plain replacement: the generator HTML-escapes the source text
/// before inserting its own tags, so a literal `<b>` in a post body arrives
/// here as `&lt;b&gt;` and is left alone.
#[must_use]
pub fn highlight_as_mark(html: &str) -> String {
    html.replace("<b>", "<mark>").replace("</b>", "</mark>")
}

/// Escapes tantivy special characters so user input is treated as plain
/// text terms (injection-ish strings cannot crash or alter the query).
#[must_use]
pub fn escape_query(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for c in input.chars() {
        if "+^`~:{ }[]!\"()".contains(c) || c == '\\' {
            out.push(' ');
        } else {
            out.push(c);
        }
    }
    out
}

impl IndexManager {
    fn searcher(&self) -> Result<Searcher, AppError> {
        self.reader
            .reload()
            .map_err(|e| AppError::internal_msg(e.to_string()))?;
        Ok(self.reader.searcher())
    }

    /// Runs a full-text query over body+title (title boosted), optionally
    /// filtered by post type; returns up to `limit` hits from `offset`.
    ///
    /// # Errors
    /// [`AppError::Internal`] on search failure. Hostile query strings are
    /// escaped and never crash the parser.
    pub fn search(
        &self,
        query_text: &str,
        post_type: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<SearchHit>, AppError> {
        let searcher = self.searcher()?;
        let (f_title, f_body, _f_type) = self.fields();
        let escaped = escape_query(query_text);

        let parser = QueryParser::for_index(&self.index, vec![f_title, f_body]);
        let query = match parser.parse_query(&escaped) {
            Ok(q) => q,
            Err(_) => Box::new(TermQuery::new(
                Term::from_field_text(f_body, &escaped.to_lowercase()),
                IndexRecordOption::Basic,
            )) as Box<dyn tantivy::query::Query>,
        };

        let top = searcher
            .search(&query, &TopDocs::with_limit(limit + offset))
            .map_err(|e| AppError::internal_msg(format!("search: {e}")))?;

        // Snippet generator bound to the same query over the body field.
        let snippets = SnippetGenerator::create(&searcher, &query, f_body).ok();

        let mut all = Vec::new();
        for (score, addr) in top {
            let _ = score;
            all.push(addr);
        }
        let mut hits = Vec::new();
        for addr in all.into_iter().skip(offset) {
            let Ok(doc) = searcher.doc::<tantivy::TantivyDocument>(addr) else {
                continue;
            };
            let id = owned_u64(doc.get_first(self.f_id_public()).map(Into::into).as_ref());
            if let Some(t) = post_type {
                let dt = owned_text(doc.get_first(self.f_type_public()).map(Into::into).as_ref());
                if dt != *t {
                    continue;
                }
            }
            let slug = owned_text(doc.get_first(self.f_slug_public()).map(Into::into).as_ref());
            let title = owned_text(
                doc.get_first(self.f_title_public())
                    .map(Into::into)
                    .as_ref(),
            );
            let body_text =
                owned_text(doc.get_first(self.f_body_public()).map(Into::into).as_ref());
            let snippet = match snippets.as_ref() {
                Some(gen) => highlight_as_mark(&gen.snippet_from_doc(&doc).to_html()),
                None => body_text.chars().take(160).collect(),
            };
            hits.push(SearchHit {
                id,
                slug,
                title,
                snippet,
            });
        }
        Ok(hits)
    }

    pub(crate) fn f_id_public(&self) -> tantivy::schema::Field {
        self.fields_id()
    }
    pub(crate) fn f_slug_public(&self) -> tantivy::schema::Field {
        self.fields_slug()
    }
    pub(crate) fn f_title_public(&self) -> tantivy::schema::Field {
        self.fields_title()
    }
    pub(crate) fn f_body_public(&self) -> tantivy::schema::Field {
        self.fields_body()
    }
    pub(crate) fn f_type_public(&self) -> tantivy::schema::Field {
        self.fields_type()
    }
}
