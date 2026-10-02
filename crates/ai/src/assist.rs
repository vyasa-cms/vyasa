//! Editor assist: per-kind PromptSpec + content extraction.
//!
//! Two families of kind live here. *Document* kinds (title, excerpt, SEO,
//! tags) read the whole post and answer with structured suggestions the
//! author copies into a form. *Text* kinds (rewrite, shorten, expand, fix,
//! continue) act on a passage the author selected — or, for `continue`, on
//! the text before the caret — and answer with prose that replaces or
//! follows it. The second family is what makes the assistant usable while
//! writing rather than after.

use vyasa_common::AppError;
use vyasa_core::block::Block;

use crate::provider::PromptSpec;

/// Assist kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssistKind {
    /// Title suggestions (3).
    Title,
    /// Excerpt suggestions (3).
    Excerpt,
    /// SEO meta title + description within length caps.
    Seo,
    /// Tag suggestions from existing vocabulary plus new proposals.
    Tags,
    /// Rewrite the selected passage for clarity, keeping meaning and length.
    Rewrite,
    /// Cut the selected passage to roughly half its length.
    Shorten,
    /// Add a sentence or two of supporting detail to the selection.
    Expand,
    /// Correct spelling, grammar and punctuation only.
    Fix,
    /// Write the next paragraph from where the document stops.
    Continue,
}

impl AssistKind {
    /// Every kind, for tests and documentation.
    pub const ALL: [Self; 9] = [
        Self::Title,
        Self::Excerpt,
        Self::Seo,
        Self::Tags,
        Self::Rewrite,
        Self::Shorten,
        Self::Expand,
        Self::Fix,
        Self::Continue,
    ];

    /// Parses the URL `{kind}` segment.
    ///
    /// # Errors
    /// Unknown kind string.
    pub fn parse(raw: &str) -> Result<Self, AppError> {
        match raw {
            "title" => Ok(Self::Title),
            "excerpt" => Ok(Self::Excerpt),
            "seo" => Ok(Self::Seo),
            "tags" => Ok(Self::Tags),
            "rewrite" => Ok(Self::Rewrite),
            "shorten" => Ok(Self::Shorten),
            "expand" => Ok(Self::Expand),
            "fix" => Ok(Self::Fix),
            "continue" => Ok(Self::Continue),
            other => Err(AppError::validation(format!(
                "unknown assist kind {other:?}"
            ))),
        }
    }

    /// Purpose tag for ai_logs.
    #[must_use]
    pub fn purpose(self) -> &'static str {
        match self {
            Self::Title => "assist-title",
            Self::Excerpt => "assist-excerpt",
            Self::Seo => "assist-seo",
            Self::Tags => "assist-tags",
            Self::Rewrite => "assist-rewrite",
            Self::Shorten => "assist-shorten",
            Self::Expand => "assist-expand",
            Self::Fix => "assist-fix",
            Self::Continue => "assist-continue",
        }
    }

    /// True for the kinds that act on a passage and answer with prose.
    #[must_use]
    pub fn is_text(self) -> bool {
        matches!(
            self,
            Self::Rewrite | Self::Shorten | Self::Expand | Self::Fix | Self::Continue
        )
    }

    /// Builds the PromptSpec for a document kind from extracted text (+
    /// optional existing tag vocabulary).
    ///
    /// Text kinds go through [`Self::text_prompt_spec`]; calling this with
    /// one produces a rewrite-style prompt with no passage, which is never
    /// what a caller wants, so it is debug-asserted.
    #[must_use]
    pub fn prompt_spec(self, content_text: &str, vocabulary: &[String]) -> PromptSpec {
        debug_assert!(!self.is_text(), "text kinds use text_prompt_spec");
        let vocab = vocabulary.join(", ");
        let (system, user) = match self {
            Self::Title => (
                String::from("You suggest 3 concise blog post titles."),
                format!("Content:\n{content_text}\nSuggest 3 titles."),
            ),
            Self::Excerpt => (
                String::from("You write 1-2 sentence excerpts in a neutral voice."),
                format!("Content:\n{content_text}\nWrite an excerpt."),
            ),
            Self::Seo => (
                String::from("You write SEO meta: title <=60 chars, description <=155 chars."),
                format!("Content:\n{content_text}\nProduce meta title and description."),
            ),
            Self::Tags => (
                format!(
                    "You suggest up to 5 tags. Prefer existing tags: {vocab}. \
                     New tags allowed with lower confidence."
                ),
                format!("Content:\n{content_text}\nSuggest tags."),
            ),
            Self::Rewrite | Self::Shorten | Self::Expand | Self::Fix | Self::Continue => {
                return self.text_prompt_spec("", content_text);
            }
        };
        let schema = match self {
            Self::Title | Self::Excerpt => {
                serde_json::json!({
                    "type": "object",
                    "properties": {"suggestions": {"type": "array", "items": {"type": "string"}}},
                    "required": ["suggestions"],
                    "additionalProperties": false
                })
            }
            Self::Seo => serde_json::json!({
                "type": "object",
                "properties": {
                    "meta_title": {"type": "string", "maxLength": 60},
                    "meta_description": {"type": "string", "maxLength": 155}
                },
                "required": ["meta_title", "meta_description"],
                "additionalProperties": false
            }),
            Self::Tags => serde_json::json!({
                "type": "object",
                "properties": {
                    "tags": {"type": "array", "items": {"type": "string"}},
                    "new_tags": {"type": "array", "items": {"type": "string"}}
                },
                "required": ["tags"],
                "additionalProperties": false
            }),
            Self::Rewrite | Self::Shorten | Self::Expand | Self::Fix | Self::Continue => {
                text_schema()
            }
        };
        PromptSpec {
            attachments: Vec::new(),
            purpose: self.purpose().to_owned(),
            model: model_name(),
            system,
            user,
            output_schema: schema,
        }
    }

    /// Builds the PromptSpec for a text kind.
    ///
    /// `passage` is the author's selection (for `continue`, the document so
    /// far); `context` is the surrounding document text, given so the model
    /// matches the piece's voice rather than a generic one. The reply is a
    /// single `text` field holding prose only — no preamble, no quotes —
    /// because the editor drops it straight into the document.
    #[must_use]
    pub fn text_prompt_spec(self, passage: &str, context: &str) -> PromptSpec {
        const VOICE: &str = "Match the document's voice, tense and register. \
             Reply with the resulting text only: no preamble, no quotation \
             marks, no commentary. Keep any inline HTML tags that are present.";
        let system = match self {
            Self::Rewrite => format!(
                "You are a line editor. Rewrite the passage so it reads more \
                 clearly, keeping its meaning, facts and approximate length. {VOICE}"
            ),
            Self::Shorten => format!(
                "You are a line editor. Cut the passage to roughly half its \
                 length without losing its key points. {VOICE}"
            ),
            Self::Expand => format!(
                "You are a co-writer. Expand the passage with one or two more \
                 sentences of supporting detail consistent with the document. \
                 Do not invent facts, names or figures. {VOICE}"
            ),
            Self::Fix => format!(
                "You are a proofreader. Correct spelling, grammar and \
                 punctuation only; do not change wording, meaning or tone. \
                 If nothing needs correcting, return the passage unchanged. {VOICE}"
            ),
            Self::Continue => format!(
                "You are a co-writer. Continue the document from where it \
                 stops with one short paragraph of two to four sentences. Do \
                 not repeat or summarise what is already written. {VOICE}"
            ),
            Self::Title | Self::Excerpt | Self::Seo | Self::Tags => {
                unreachable!("document kinds use prompt_spec")
            }
        };
        let user = if self == Self::Continue {
            format!("Document so far:\n{passage}")
        } else {
            format!("Document (for context only):\n{context}\n\nPassage to work on:\n{passage}")
        };
        PromptSpec {
            attachments: Vec::new(),
            purpose: self.purpose().to_owned(),
            model: model_name(),
            system,
            user,
            output_schema: text_schema(),
        }
    }
}

fn model_name() -> String {
    std::env::var("VYASA_AI_MODEL").unwrap_or_else(|_| String::from("claude-haiku-4-5"))
}

fn text_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {"text": {"type": "string"}},
        "required": ["text"],
        "additionalProperties": false
    })
}

/// Extracts plain text from a block document for prompting: paragraphs +
/// headings only, truncated to `budget` characters with ellipsis.
#[must_use]
pub fn extract_text(blocks: &[Block], budget: usize) -> String {
    fn walk<'a>(blocks: &'a [Block], out: &'a mut String) -> &'a mut String {
        for b in blocks {
            if b.kind.as_str() == "paragraph" || b.kind.as_str() == "heading" {
                if let Some(text) = b.attrs.get("text").and_then(serde_json::Value::as_str) {
                    out.push_str(text);
                    out.push('\n');
                }
            }
            walk(&b.children, out);
        }
        out
    }
    let mut buf = String::new();
    walk(blocks, &mut buf);
    let trimmed = buf.trim().to_owned();
    if trimmed.chars().count() > budget {
        let cut: String = trimmed.chars().take(budget).collect();
        format!("{cut}…")
    } else {
        trimmed
    }
}

/// The tail of a document, for `continue`: the model needs what comes
/// right before the caret far more than the opening.
#[must_use]
pub fn tail(text: &str, budget: usize) -> String {
    let count = text.chars().count();
    if count <= budget {
        return text.to_owned();
    }
    let skipped: String = text.chars().skip(count - budget).collect();
    format!("…{skipped}")
}

/// Whether extraction found enough content to prompt on.
#[must_use]
pub fn enough_content(text: &str) -> bool {
    text.chars().count() >= 30
}

/// Whether a selection is worth sending: a single word can be rewritten,
/// but whitespace or a stray character cannot.
#[must_use]
pub fn enough_passage(text: &str) -> bool {
    text.trim().chars().count() >= 3
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_parses_from_its_url_segment_and_back() {
        for kind in AssistKind::ALL {
            let segment = kind.purpose().trim_start_matches("assist-");
            assert_eq!(AssistKind::parse(segment).unwrap(), kind);
        }
        assert!(AssistKind::parse("summarise").is_err());
    }

    #[test]
    fn purposes_are_distinct_so_usage_logs_can_tell_them_apart() {
        let mut seen = std::collections::HashSet::new();
        for kind in AssistKind::ALL {
            assert!(seen.insert(kind.purpose()), "{kind:?} shares a purpose");
        }
    }

    #[test]
    fn text_kinds_prompt_on_the_passage_and_answer_with_text() {
        let spec = AssistKind::Rewrite.text_prompt_spec("the passage", "the doc");
        assert!(spec.user.contains("the passage"));
        assert!(spec.user.contains("the doc"));
        assert_eq!(spec.output_schema["required"], serde_json::json!(["text"]));
        assert_eq!(spec.purpose, "assist-rewrite");
    }

    #[test]
    fn continue_sees_only_the_document_so_far() {
        let spec = AssistKind::Continue.text_prompt_spec("so far", "ignored");
        assert!(spec.user.contains("so far"));
        assert!(!spec.user.contains("ignored"));
        assert!(spec.system.contains("Do not repeat"));
    }

    #[test]
    fn document_kinds_keep_their_structured_schemas() {
        let spec = AssistKind::Tags.prompt_spec("content", &["rust".to_owned()]);
        assert!(spec.system.contains("rust"));
        assert_eq!(spec.output_schema["required"], serde_json::json!(["tags"]));
        assert!(!AssistKind::Tags.is_text());
        assert!(AssistKind::Fix.is_text());
    }

    #[test]
    fn tail_keeps_the_end_and_marks_the_cut() {
        assert_eq!(tail("abcdef", 10), "abcdef");
        assert_eq!(tail("abcdef", 3), "…def");
    }

    #[test]
    fn passage_threshold_rejects_noise_but_not_a_word() {
        assert!(!enough_passage("  a "));
        assert!(enough_passage("word"));
    }
}
