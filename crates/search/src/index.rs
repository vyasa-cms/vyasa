//! Tantivy index: schema, writer, batch upsert/delete.

use std::path::Path;

use tantivy::schema::{Field, Schema, STORED, STRING, TEXT};
use tantivy::{Index, IndexWriter, Term};
use vyasa_common::AppError;

/// One searchable document.
#[derive(Debug, Clone)]
pub struct SearchDoc {
    /// Post id (u64 for tantivy u64 field).
    pub id: u64,
    /// Post type (`post`/`page`).
    pub post_type: String,
    /// Slug.
    pub slug: String,
    /// Title.
    pub title: String,
    /// Body text extracted from blocks.
    pub body: String,
}

/// Removes an index directory's contents, leaving the directory itself.
///
/// Only ever called on a directory tantivy has identified as an index, so
/// this cannot wander into unrelated data.
fn clear_dir(dir: &Path) -> Result<(), AppError> {
    let entries =
        std::fs::read_dir(dir).map_err(|e| AppError::internal_msg(format!("read index: {e}")))?;
    for entry in entries.flatten() {
        let path = entry.path();
        let result = if path.is_dir() {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        };
        result
            .map_err(|e| AppError::internal_msg(format!("clear index {}: {e}", path.display())))?;
    }
    Ok(())
}

/// The index manager: single-process writer + reloaded reader.
pub struct IndexManager {
    pub(crate) index: Index,
    writer: std::sync::Mutex<IndexWriter>,
    pub(crate) reader: tantivy::IndexReader,
    f_id: Field,
    f_type: Field,
    f_slug: Field,
    f_title: Field,
    f_body: Field,
}

impl IndexManager {
    /// Opens (or creates) the index at `dir`.
    ///
    /// # Errors
    /// [`AppError::Internal`] on IO/schema failures.
    pub fn open(dir: &Path) -> Result<Self, AppError> {
        std::fs::create_dir_all(dir)
            .map_err(|e| AppError::internal_msg(format!("index dir: {e}")))?;
        let mut schema = Schema::builder();
        let numeric_opts = tantivy::schema::NumericOptions::default()
            .set_stored()
            .set_indexed();
        let f_id = schema.add_u64_field("id", numeric_opts);
        let f_type = schema.add_text_field("type", STRING);
        let f_slug = schema.add_text_field("slug", STORED | STRING);
        // Title and body are STORED as well as indexed. Tantivy's snippet
        // generator reads the *stored* text to build a highlight, so with
        // plain TEXT it silently produced an empty snippet for every hit —
        // which is what shipped. Storing them costs index size; the feature
        // does not work without it.
        let f_title = schema.add_text_field("title", TEXT | STORED);
        let f_body = schema.add_text_field("body", TEXT | STORED);
        let schema = schema.build();

        // An index written with an older schema cannot answer with the new
        // fields, and tantivy will happily open it and return nothing for
        // them. Detect the mismatch and start over rather than serve empty
        // results forever; content is reindexed from the database.
        let index = match Index::open_in_dir(dir) {
            Ok(existing) if existing.schema() == schema => existing,
            Ok(_) => {
                tracing::warn!(
                    "search index schema changed; rebuilding {}. Run `vyasa search reindex` \
                     or POST /api/v1/search/reindex to repopulate it.",
                    dir.display()
                );
                clear_dir(dir)?;
                Index::create_in_dir(dir, schema.clone())
                    .map_err(|e| AppError::internal_msg(format!("index recreate: {e}")))?
            }
            Err(_) => Index::create_in_dir(dir, schema.clone())
                .map_err(|e| AppError::internal_msg(format!("index create: {e}")))?,
        };
        let writer = std::sync::Mutex::new(
            index
                .writer(15_000_000)
                .map_err(|e| AppError::internal_msg(format!("index writer: {e}")))?,
        );
        let reader = index
            .reader_builder()
            .reload_policy(tantivy::ReloadPolicy::Manual)
            .try_into()
            .map_err(|e| AppError::internal_msg(format!("reader init: {e}")))?;
        Ok(Self {
            index,
            writer,
            reader,
            f_id,
            f_type,
            f_slug,
            f_title,
            f_body,
        })
    }

    fn doc(&self, d: &SearchDoc) -> tantivy::TantivyDocument {
        let mut doc = tantivy::TantivyDocument::default();
        doc.add_u64(self.f_id, d.id);
        doc.add_text(self.f_type, &d.post_type);
        doc.add_text(self.f_slug, &d.slug);
        doc.add_text(self.f_title, &d.title);
        doc.add_text(self.f_body, &d.body);
        doc
    }

    /// Upserts one document (delete-by-id then add).
    ///
    /// # Errors
    /// [`AppError::Internal`] on write failure.
    pub fn upsert(&self, doc: &SearchDoc) -> Result<(), AppError> {
        let mut writer = self
            .writer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        writer.delete_term(Term::from_field_u64(self.f_id, doc.id));
        writer
            .add_document(self.doc(doc))
            .map_err(|e| AppError::internal_msg(format!("add doc: {e}")))?;
        writer
            .commit()
            .map_err(|e| AppError::internal_msg(format!("commit: {e}")))?;
        self.index
            .reader()
            .map_err(|e| AppError::internal_msg(format!("reload reader: {e}")))?
            .reload()
            .map_err(|e| AppError::internal_msg(format!("reader reload: {e}")))?;
        Ok(())
    }

    /// Number of documents currently searchable.
    ///
    /// Read from the reader rather than the writer so it reflects what a
    /// query would actually find, including pending deletes.
    #[must_use]
    pub fn doc_count(&self) -> u64 {
        self.reader.searcher().num_docs()
    }

    /// Deletes by post id.
    ///
    /// # Errors
    /// [`AppError::Internal`] on write failure.
    pub fn delete(&self, id: u64) -> Result<(), AppError> {
        let mut writer = self
            .writer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        writer.delete_term(Term::from_field_u64(self.f_id, id));
        writer
            .commit()
            .map_err(|e| AppError::internal_msg(format!("commit delete: {e}")))?;
        self.reader.reload().ok();
        Ok(())
    }

    /// Fields for the query module.
    pub(crate) fn fields(&self) -> (Field, Field, Field) {
        (self.f_title, self.f_body, self.f_type)
    }
}

impl IndexManager {
    pub(crate) fn fields_id(&self) -> Field {
        self.f_id
    }
    pub(crate) fn fields_slug(&self) -> Field {
        self.f_slug
    }
    pub(crate) fn fields_title(&self) -> Field {
        self.f_title
    }
    pub(crate) fn fields_body(&self) -> Field {
        self.f_body
    }
    pub(crate) fn fields_type(&self) -> Field {
        self.f_type
    }
}
