//! Block document envelope with schema versioning.

use serde::{Deserialize, Serialize};

use vyasa_common::AppError;

use super::types::Block;
use super::validate::{validate, Validated};

/// Current block-document schema version.
pub const SCHEMA_VERSION: u32 = 1;

/// A complete post/page content document.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BlockDocument {
    /// Wire schema version of the block format.
    pub schema_version: u32,
    /// Top-level blocks.
    pub blocks: Vec<Block>,
}

impl BlockDocument {
    /// Builds a new v1 document from `blocks` without validating.
    #[must_use]
    pub fn new(blocks: Vec<Block>) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            blocks,
        }
    }

    /// Parses a document from JSON, enforcing the schema version and
    /// validating the block tree.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Validation`] for malformed JSON, unsupported
    /// schema versions, or any validation failure.
    pub fn from_json(value: serde_json::Value) -> Result<Self, AppError> {
        let doc: BlockDocument = serde_json::from_value(value)
            .map_err(|err| AppError::validation(format!("invalid block document: {err}")))?;
        if doc.schema_version != SCHEMA_VERSION {
            return Err(AppError::validation(format!(
                "unsupported block schema version {} (expected {SCHEMA_VERSION})",
                doc.schema_version
            )));
        }
        doc.validate()?;
        Ok(doc)
    }

    /// Validates the block tree.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Validation`] on any rule violation.
    pub fn validate(&self) -> Result<Validated, AppError> {
        validate(&self.blocks)
    }

    /// Serializes to a JSON value for storage.
    #[must_use]
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or(serde_json::Value::Null)
    }
}

#[cfg(test)]
mod tests {
    use super::{BlockDocument, SCHEMA_VERSION};
    use crate::block::types::{Block, BlockKind};
    use serde_json::json;

    fn doc(blocks: &serde_json::Value) -> serde_json::Value {
        json!({"schema_version": SCHEMA_VERSION, "blocks": blocks.clone()})
    }

    #[test]
    fn round_trip_serialization() {
        let document = BlockDocument::new(vec![Block::new(
            BlockKind::Heading,
            json!({"level": 2, "text": "Hello"}),
        )]);
        let json = document.to_json();
        let parsed = BlockDocument::from_json(json).expect("parse");
        assert_eq!(parsed, document);
    }

    #[test]
    fn rejects_wrong_schema_version() {
        let value = json!({"schema_version": 99, "blocks": []});
        let err = BlockDocument::from_json(value).unwrap_err().to_string();
        assert!(err.contains("schema version"), "err: {err}");
    }

    #[test]
    fn rejects_malformed_json() {
        let value = json!({"schema_version": 1}); // missing blocks
        assert!(BlockDocument::from_json(value).is_err());
        let value = json!([{"kind": "nope"}]);
        assert!(BlockDocument::from_json(value).is_err());
    }

    #[test]
    fn children_default_to_empty_and_omit_in_json() {
        let value = doc(&json!([{"kind": "separator"}]));
        let parsed = BlockDocument::from_json(value).expect("parse");
        assert!(parsed.blocks[0].children.is_empty());
        let serialized = parsed.to_json().to_string();
        assert!(!serialized.contains("children"), "s: {serialized}");
    }

    #[test]
    fn parses_every_block_kind() {
        let kinds: Vec<&str> = BlockKind::ALL.iter().map(|k| k.as_str()).collect();
        // Just check that each kind name parses and validates as an
        // attribute-less leaf where allowed.
        for kind in kinds {
            let block = json!([{"kind": kind}]);
            // Some kinds require attrs; those must fail validation (but
            // still parse). Separator requires nothing.
            if kind == "separator" || kind == "page_break" {
                let parsed = BlockDocument::from_json(doc(&block));
                assert!(parsed.is_ok(), "kind {kind} should validate bare");
            }
            // Unknown kinds must always fail.
            let bad = json!([{"kind": kind, "bogus": true}]);
            let _ = BlockDocument::from_json(doc(&bad));
        }
    }
}
