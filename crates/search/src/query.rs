//! Query parsing, execution, and snippet generation.

use tantivy::collector::TopDocs;
use tantivy::query::{BooleanQuery, Occur, Query, QueryParser, TermQuery};
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

/// What a search is limited to. Both parts are exact and optional; a hit
/// must satisfy every part that is set.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Scope {
    /// Only this post type (`doc`).
    pub post_type: Option<String>,
    /// Only entries filed under this term, as `taxonomy:slug`
    /// (`doc-version:v2`).
    pub term: Option<String>,
}

impl Scope {
    /// Whether nothing is limited.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.post_type.is_none() && self.term.is_none()
    }
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
        let scope = Scope {
            post_type: post_type.map(str::to_owned),
            term: None,
        };
        self.search_scoped(query_text, &scope, limit, offset)
    }

    /// [`IndexManager::search`] limited to a [`Scope`].
    ///
    /// The scope is part of the query, not a filter over its results: a
    /// filter applied after the top `limit` hits were chosen returns fewer
    /// than `limit` (often none) whenever the best matches are elsewhere.
    ///
    /// # Errors
    /// [`AppError::Internal`] on search failure.
    pub fn search_scoped(
        &self,
        query_text: &str,
        scope: &Scope,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<SearchHit>, AppError> {
        let searcher = self.searcher()?;
        let (f_title, f_body, f_type) = self.fields();
        let escaped = escape_query(query_text);

        let parser = QueryParser::for_index(&self.index, vec![f_title, f_body]);
        let text_query = match parser.parse_query(&escaped) {
            Ok(q) => q,
            Err(_) => Box::new(TermQuery::new(
                Term::from_field_text(f_body, &escaped.to_lowercase()),
                IndexRecordOption::Basic,
            )) as Box<dyn Query>,
        };
        let mut clauses: Vec<(Occur, Box<dyn Query>)> = vec![(Occur::Must, text_query.box_clone())];
        if let Some(t) = &scope.post_type {
            clauses.push((
                Occur::Must,
                Box::new(TermQuery::new(
                    Term::from_field_text(f_type, t),
                    IndexRecordOption::Basic,
                )),
            ));
        }
        if let Some(t) = &scope.term {
            clauses.push((
                Occur::Must,
                Box::new(TermQuery::new(
                    Term::from_field_text(self.fields_terms(), t),
                    IndexRecordOption::Basic,
                )),
            ));
        }
        let query: Box<dyn Query> = if clauses.len() == 1 {
            text_query.box_clone()
        } else {
            Box::new(BooleanQuery::new(clauses))
        };

        let top = searcher
            .search(
                &query,
                &TopDocs::with_limit(limit + offset).order_by_score(),
            )
            .map_err(|e| AppError::internal_msg(format!("search: {e}")))?;

        // Snippets are highlighted against the text query alone: the scope
        // clauses name no words.
        let snippets = SnippetGenerator::create(&searcher, &*text_query, f_body).ok();

        let mut hits = Vec::new();
        for (_score, addr) in top.into_iter().skip(offset) {
            let Ok(doc) = searcher.doc::<tantivy::TantivyDocument>(addr) else {
                continue;
            };
            let id = owned_u64(doc.get_first(self.f_id_public()).map(Into::into).as_ref());
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
}

#[cfg(test)]
mod scope_tests {
    use super::*;
    use crate::index::SearchDoc;

    fn doc(id: u64, ty: &str, title: &str, terms: &[&str]) -> SearchDoc {
        SearchDoc {
            id,
            post_type: ty.to_owned(),
            slug: format!("s{id}"),
            title: title.to_owned(),
            body: if title.starts_with("Configure") {
                "configure the thing".to_owned()
            } else {
                "install the thing".to_owned()
            },
            terms: terms.iter().map(|t| (*t).to_owned()).collect(),
        }
    }

    fn index() -> (IndexManager, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let m = IndexManager::open(dir.path()).expect("open");
        for d in [
            doc(1, "post", "Install notes", &["category:news"]),
            doc(2, "doc", "Install guide v2", &["doc-version:v2"]),
            doc(3, "doc", "Install guide v1", &["doc-version:v1"]),
            doc(4, "doc", "Configure", &["doc-version:v2", "category:ref"]),
        ] {
            m.upsert(&d).expect("upsert");
        }
        (m, dir)
    }

    fn ids(hits: Vec<SearchHit>) -> Vec<u64> {
        let mut v: Vec<u64> = hits.into_iter().map(|h| h.id).collect();
        v.sort_unstable();
        v
    }

    #[test]
    fn an_empty_scope_searches_everything() {
        let (m, _d) = index();
        let all = m
            .search_scoped("install", &Scope::default(), 10, 0)
            .unwrap();
        assert_eq!(ids(all), [1, 2, 3]);
    }

    #[test]
    fn a_type_scope_limits_the_query_not_its_results() {
        let (m, _d) = index();
        let scope = Scope {
            post_type: Some("doc".into()),
            term: None,
        };
        assert_eq!(
            ids(m.search_scoped("install", &scope, 10, 0).unwrap()),
            [2, 3]
        );
        // The old behaviour filtered after taking the top `limit`: with a
        // limit of 1 the best hit may be the post, and the doc search came
        // back empty. A scoped query returns the best *doc*.
        let one = m.search_scoped("install", &scope, 1, 0).unwrap();
        assert_eq!(one.len(), 1);
        assert!([2, 3].contains(&one[0].id));
        assert_eq!(m.search("install", Some("doc"), 1, 0).unwrap().len(), 1);
    }

    #[test]
    fn a_term_scope_matches_exactly() {
        let (m, _d) = index();
        let v2 = Scope {
            post_type: Some("doc".into()),
            term: Some("doc-version:v2".into()),
        };
        assert_eq!(ids(m.search_scoped("install", &v2, 10, 0).unwrap()), [2]);
        let any_v2 = Scope {
            post_type: None,
            term: Some("doc-version:v2".into()),
        };
        assert_eq!(
            ids(m.search_scoped("thing", &any_v2, 10, 0).unwrap()),
            [2, 4]
        );
        let none = Scope {
            post_type: None,
            term: Some("doc-version:v".into()),
        };
        assert!(m.search_scoped("install", &none, 10, 0).unwrap().is_empty());
        let other_tax = Scope {
            post_type: None,
            term: Some("category:v2".into()),
        };
        assert!(m
            .search_scoped("install", &other_tax, 10, 0)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn snippets_still_highlight_under_a_scope() {
        let (m, _d) = index();
        let scope = Scope {
            post_type: Some("doc".into()),
            term: Some("doc-version:v1".into()),
        };
        let hits = m.search_scoped("configure", &scope, 10, 0).unwrap();
        assert!(
            hits.is_empty(),
            "the only v1 doc says install, not configure"
        );
        let hits = m.search_scoped("thing", &scope, 10, 0).unwrap();
        assert_eq!(hits.len(), 1);
        assert!(
            hits[0].snippet.contains("<mark>thing</mark>"),
            "{}",
            hits[0].snippet
        );
    }

    #[test]
    fn hostile_input_is_still_escaped() {
        let (m, _d) = index();
        let scope = Scope {
            post_type: Some("doc".into()),
            term: None,
        };
        assert!(m.search_scoped("type:post OR (", &scope, 10, 0).is_ok());
    }

    #[test]
    fn an_old_schema_is_rebuilt_empty() {
        // A stand-in for an index written before the `terms` field existed:
        // open() must recreate it rather than answer from a stale schema.
        let dir = tempfile::tempdir().unwrap();
        {
            let mut b = tantivy::schema::Schema::builder();
            b.add_u64_field(
                "id",
                tantivy::schema::NumericOptions::default()
                    .set_stored()
                    .set_indexed(),
            );
            tantivy::Index::create_in_dir(dir.path(), b.build()).unwrap();
        }
        let m = IndexManager::open(dir.path()).unwrap();
        assert_eq!(m.doc_count(), 0);
        m.upsert(&doc(9, "doc", "Fresh", &["doc-version:v2"]))
            .unwrap();
        let scope = Scope {
            post_type: None,
            term: Some("doc-version:v2".into()),
        };
        assert_eq!(m.search_scoped("fresh", &scope, 5, 0).unwrap().len(), 1);
    }
}
