//! Term service with hierarchy, cycle prevention, and merge.

use vyasa_common::{slugify, AppError};
use vyasa_db::content_models::{Taxonomy, TermRow};
use vyasa_db::repo::{TermWithCount, TermsRepo};

/// Term management.
#[derive(Clone, Debug)]
pub struct TermService {
    terms: TermsRepo,
    posts: vyasa_db::repo::PostsRepo,
}

impl TermService {
    /// Creates a service over the given repos.
    #[must_use]
    pub fn new(terms: TermsRepo, posts: vyasa_db::repo::PostsRepo) -> Self {
        Self { terms, posts }
    }

    /// Creates a term. Derives slug from name when absent.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Validation`] for empty name/slug,
    /// [`AppError::Conflict`] when slug taken, [`AppError::Db`] otherwise.
    pub async fn create(
        &self,
        taxonomy: Taxonomy,
        name: &str,
        slug: Option<&str>,
        parent_id: Option<i64>,
        meta: Option<serde_json::Value>,
    ) -> Result<TermRow, AppError> {
        let name = name.trim().to_string();
        if name.is_empty() {
            return Err(AppError::validation("term name must not be empty"));
        }
        let slug = if let Some(s) = slug {
            let s = slugify(s);
            if s.is_empty() {
                return Err(AppError::validation("term slug must not be empty"));
            }
            s
        } else {
            let s = slugify(&name);
            if s.is_empty() {
                return Err(AppError::validation(
                    "cannot derive slug from term name; provide one",
                ));
            }
            s
        };
        if let Some(parent) = parent_id {
            let parent_term = self.terms.get(parent).await?;
            if parent_term.taxonomy != taxonomy {
                return Err(AppError::validation(
                    "parent term must be in the same taxonomy",
                ));
            }
            // No cycle possible for new term (no descendants yet), but check parent exists.
        }
        let meta = meta.unwrap_or_else(|| serde_json::json!({}));
        let id = vyasa_common::next_id_i64();
        match self
            .terms
            .insert(&vyasa_db::repo::NewTerm {
                id,
                taxonomy,
                name: &name,
                slug: &slug,
                parent_id,
                meta,
            })
            .await
        {
            Ok(term) => Ok(term),
            Err(AppError::Db { message }) if message.contains("duplicate key") => {
                Err(AppError::conflict(format!(
                    "slug {slug:?} is already in use in taxonomy {}",
                    taxonomy.as_str()
                )))
            }
            Err(err) => Err(err),
        }
    }

    /// Fetches a term by id.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::NotFound`] when missing.
    pub async fn get(&self, id: i64) -> Result<TermRow, AppError> {
        self.terms.get(id).await
    }

    /// Lists terms, optionally filtered and with post counts.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn list(
        &self,
        taxonomy: Option<Taxonomy>,
        with_counts: bool,
    ) -> Result<Vec<TermWithCount>, AppError> {
        self.terms.list(taxonomy, with_counts).await
    }

    /// Renames / reslugs / reparents a term.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Validation`] for empty name/slug or cycle,
    /// [`AppError::Conflict`] for duplicate slug, [`AppError::NotFound`] when missing.
    pub async fn update(
        &self,
        id: i64,
        name: Option<&str>,
        slug: Option<&str>,
        parent_id: Option<Option<i64>>,
        meta: Option<serde_json::Value>,
    ) -> Result<TermRow, AppError> {
        let current = self.terms.get(id).await?;
        let name = match name {
            Some(n) => {
                let n = n.trim().to_string();
                if n.is_empty() {
                    return Err(AppError::validation("term name must not be empty"));
                }
                Some(n)
            }
            None => None,
        };
        let slug = match slug {
            Some(s) => {
                let s = slugify(s);
                if s.is_empty() {
                    return Err(AppError::validation("term slug must not be empty"));
                }
                Some(s)
            }
            None => None,
        };
        // Validate the new parent up front for a clear error; the repo
        // re-checks the cycle and writes parent + fields in one
        // transaction, so a refused slug leaves the parent untouched.
        if let Some(Some(pid)) = parent_id {
            if pid == id {
                return Err(AppError::validation("term cannot be its own parent"));
            }
            let parent = self.terms.get(pid).await?;
            if parent.taxonomy != current.taxonomy {
                return Err(AppError::validation(
                    "parent term must be in the same taxonomy",
                ));
            }
        }
        self.terms
            .update_with_parent(
                id,
                &vyasa_db::repo::TermUpdate {
                    name: name.as_deref(),
                    slug: slug.as_deref(),
                    meta: meta.as_ref(),
                },
                parent_id,
            )
            .await
    }

    /// Deletes a term, optionally reassigning its posts and children.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Validation`] when the target is this term, in
    /// another taxonomy, or one of its descendants (its children would
    /// become the target's ancestors: a cycle); [`AppError::Db`] on
    /// database failure.
    pub async fn delete(&self, id: i64, reassign_to: Option<i64>) -> Result<(), AppError> {
        if let Some(target) = reassign_to {
            if target == id {
                return Err(AppError::validation("cannot reassign to self"));
            }
            let target_term = self.terms.get(target).await?;
            let current = self.terms.get(id).await?;
            if target_term.taxonomy != current.taxonomy {
                return Err(AppError::validation(
                    "reassign target must be in the same taxonomy",
                ));
            }
        }
        self.terms.delete(id, reassign_to).await
    }

    /// Merges `from` into `into` by moving relationships and deleting `from`.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Validation`] for a merge into itself, across
    /// taxonomies, or into one of its own descendants;
    /// [`AppError::Db`] on database failure.
    pub async fn merge(&self, from: i64, into: i64) -> Result<(), AppError> {
        if from == into {
            return Err(AppError::validation("cannot merge a term into itself"));
        }
        let from_term = self.terms.get(from).await?;
        let into_term = self.terms.get(into).await?;
        if from_term.taxonomy != into_term.taxonomy {
            return Err(AppError::validation(
                "cannot merge terms from different taxonomies",
            ));
        }
        self.terms.merge(from, into).await
    }

    /// Replaces a post's terms with `term_ids` (replace-set semantics).
    ///
    /// Validates that each term exists and that all terms are from the
    /// allowed taxonomies (when `allowed_taxonomy` is Some, only that
    /// taxonomy is permitted; otherwise any).
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Validation`] for unknown terms, [`AppError::Db`] otherwise.
    pub async fn set_post_terms(
        &self,
        post_id: i64,
        term_ids: &[i64],
        allowed_taxonomy: Option<Taxonomy>,
    ) -> Result<(), AppError> {
        // Verify post exists.
        self.posts.get(post_id).await?;
        // Validate each term.
        for term_id in term_ids {
            let term = self.terms.get(*term_id).await?;
            if let Some(tax) = allowed_taxonomy {
                if term.taxonomy != tax {
                    return Err(AppError::validation(format!(
                        "term {} is not in taxonomy {}",
                        term_id,
                        tax.as_str()
                    )));
                }
            }
        }
        // Deduplicate.
        let mut deduped = term_ids.to_vec();
        deduped.sort_unstable();
        deduped.dedup();
        self.terms.set_post_terms(post_id, &deduped).await
    }

    /// Lists term ids for a post.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Db`] on database failure.
    pub async fn list_for_post(&self, post_id: i64) -> Result<Vec<i64>, AppError> {
        // Ensure post exists for 404 instead of empty.
        self.posts.get(post_id).await?;
        self.terms.list_for_post(post_id).await
    }
}
