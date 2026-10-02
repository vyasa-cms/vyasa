//! The vocabulary: providers and the kinds of job a model can be registered
//! for. Both are closed sets on purpose — every kind here is something the
//! product knows how to call, and every provider has a client.

use vyasa_common::AppError;

/// A cloud provider the registry knows how to talk to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProviderId {
    /// Anthropic Messages API.
    Anthropic,
    /// OpenAI platform API.
    OpenAi,
    /// OpenRouter: an OpenAI-compatible gateway to many vendors' models.
    OpenRouter,
    /// Google Gemini, through its OpenAI-compatible endpoint.
    Gemini,
    /// Mistral, through its OpenAI-compatible endpoint.
    Mistral,
    /// Anything that speaks the OpenAI API: Ollama, LM Studio, vLLM, a
    /// gateway. Needs a base URL; the key may be empty.
    Custom,
}

/// A sensible model to start with, per provider and kind.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
pub struct Recommended {
    /// The provider's model id.
    pub model: &'static str,
    /// A label for menus.
    pub label: &'static str,
    /// USD per million input tokens, when known.
    pub input_cost_per_mtok: Option<f64>,
    /// USD per million output tokens, when known.
    pub output_cost_per_mtok: Option<f64>,
}

const fn rec(model: &'static str, label: &'static str, i: f64, o: f64) -> Recommended {
    Recommended {
        model,
        label,
        input_cost_per_mtok: Some(i),
        output_cost_per_mtok: Some(o),
    }
}

impl ProviderId {
    /// Every provider, in display order.
    pub const ALL: [Self; 6] = [
        Self::Anthropic,
        Self::OpenAi,
        Self::OpenRouter,
        Self::Gemini,
        Self::Mistral,
        Self::Custom,
    ];

    /// The one source of truth for "which model should I start with":
    /// the wizard, the Add dialog's placeholder and the "Use recommended"
    /// button all read it.
    #[must_use]
    pub fn recommended(self, kind: ModelKind) -> Option<Recommended> {
        use ModelKind as K;
        Some(match (self, kind) {
            (Self::Anthropic, K::Text | K::Vision) => {
                rec("claude-sonnet-5", "Claude Sonnet 5", 3.0, 15.0)
            }
            (Self::OpenAi, K::Text | K::Vision) => rec("gpt-5-mini", "GPT-5 mini", 0.25, 2.0),
            (Self::OpenAi, K::Embedding) => {
                rec("text-embedding-3-small", "Embedding 3 small", 0.02, 0.0)
            }
            (Self::OpenAi, K::Image) => rec("gpt-image-1", "GPT Image 1", 5.0, 40.0),
            (Self::OpenAi, K::Moderation) => {
                rec("omni-moderation-latest", "Omni moderation", 0.0, 0.0)
            }
            (Self::OpenAi, K::Transcription) => {
                rec("gpt-4o-mini-transcribe", "Mini transcribe", 1.25, 5.0)
            }
            (Self::OpenAi, K::Speech) => rec("gpt-4o-mini-tts", "Mini TTS", 0.6, 12.0),
            (Self::OpenRouter, K::Text | K::Vision) => rec(
                "anthropic/claude-sonnet-5",
                "Claude Sonnet 5 via OpenRouter",
                3.0,
                15.0,
            ),
            (Self::OpenRouter, K::Embedding) => rec(
                "openai/text-embedding-3-small",
                "Embedding 3 small via OpenRouter",
                0.02,
                0.0,
            ),
            (Self::OpenRouter, K::Image) => rec(
                "google/gemini-2.5-flash-image",
                "Gemini Flash Image via OpenRouter",
                0.3,
                2.5,
            ),
            (Self::Gemini, K::Text | K::Vision) => {
                rec("gemini-2.5-flash", "Gemini 2.5 Flash", 0.3, 2.5)
            }
            (Self::Gemini, K::Embedding) => {
                rec("gemini-embedding-001", "Gemini Embedding", 0.15, 0.0)
            }
            (Self::Mistral, K::Text | K::Vision) => {
                rec("mistral-medium-latest", "Mistral Medium", 0.4, 2.0)
            }
            (Self::Mistral, K::Embedding) => rec("mistral-embed", "Mistral Embed", 0.1, 0.0),
            (Self::Mistral, K::Moderation) => {
                rec("mistral-moderation-latest", "Mistral Moderation", 0.0, 0.0)
            }
            _ => return None,
        })
    }

    /// Whether a key is required to talk to it.
    #[must_use]
    pub fn needs_key(self) -> bool {
        !matches!(self, Self::Custom)
    }

    /// Whether a base URL must be given (no sensible default).
    #[must_use]
    pub fn needs_base_url(self) -> bool {
        matches!(self, Self::Custom)
    }

    /// Parses the stored/URL form.
    ///
    /// # Errors
    /// Unknown provider name.
    pub fn parse(raw: &str) -> Result<Self, AppError> {
        match raw {
            "anthropic" => Ok(Self::Anthropic),
            "openai" => Ok(Self::OpenAi),
            "openrouter" => Ok(Self::OpenRouter),
            "gemini" => Ok(Self::Gemini),
            "mistral" => Ok(Self::Mistral),
            "custom" => Ok(Self::Custom),
            other => Err(AppError::validation(format!(
                "unknown provider {other:?}; known providers are anthropic, openai, openrouter, gemini, mistral, custom"
            ))),
        }
    }

    /// Stored/URL form.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::OpenAi => "openai",
            Self::OpenRouter => "openrouter",
            Self::Gemini => "gemini",
            Self::Mistral => "mistral",
            Self::Custom => "custom",
        }
    }

    /// Display name.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Anthropic => "Anthropic",
            Self::OpenAi => "OpenAI",
            Self::OpenRouter => "OpenRouter",
            Self::Gemini => "Google Gemini",
            Self::Mistral => "Mistral",
            Self::Custom => "OpenAI-compatible",
        }
    }

    /// The API root used when no override is stored.
    #[must_use]
    pub fn default_base_url(self) -> &'static str {
        match self {
            Self::Anthropic => "https://api.anthropic.com/v1",
            Self::OpenAi => "https://api.openai.com/v1",
            Self::OpenRouter => "https://openrouter.ai/api/v1",
            Self::Gemini => "https://generativelanguage.googleapis.com/v1beta/openai",
            Self::Mistral => "https://api.mistral.ai/v1",
            Self::Custom => "http://127.0.0.1:11434/v1",
        }
    }

    /// Environment variable consulted when no key is stored.
    #[must_use]
    pub fn env_key(self) -> &'static str {
        match self {
            Self::Anthropic => "VYASA_AI_ANTHROPIC_KEY",
            Self::OpenAi => "VYASA_AI_OPENAI_KEY",
            Self::OpenRouter => "VYASA_AI_OPENROUTER_KEY",
            Self::Gemini => "VYASA_AI_GEMINI_KEY",
            Self::Mistral => "VYASA_AI_MISTRAL_KEY",
            Self::Custom => "VYASA_AI_CUSTOM_KEY",
        }
    }

    /// Kinds of job this provider's API can serve.
    #[must_use]
    pub fn supports(self, kind: ModelKind) -> bool {
        use ModelKind as K;
        match self {
            Self::Anthropic => matches!(kind, K::Text | K::Vision),
            Self::OpenAi | Self::Custom => true,
            Self::OpenRouter => matches!(kind, K::Text | K::Vision | K::Image | K::Embedding),
            Self::Gemini => matches!(kind, K::Text | K::Vision | K::Embedding),
            Self::Mistral => matches!(kind, K::Text | K::Vision | K::Embedding | K::Moderation),
        }
    }
}

/// The kind of job a registered model does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModelKind {
    /// Chat / text generation: the writing assistant, the theme builder.
    Text,
    /// Image understanding: alt text and image descriptions.
    Vision,
    /// Image generation.
    Image,
    /// Text embeddings: semantic search and related posts.
    Embedding,
    /// Content moderation: comment screening.
    Moderation,
    /// Speech to text.
    Transcription,
    /// Text to speech.
    Speech,
}

impl ModelKind {
    /// Every kind, in display order.
    pub const ALL: [Self; 7] = [
        Self::Text,
        Self::Vision,
        Self::Image,
        Self::Embedding,
        Self::Moderation,
        Self::Transcription,
        Self::Speech,
    ];

    /// Parses the stored/URL form.
    ///
    /// # Errors
    /// Unknown kind name.
    pub fn parse(raw: &str) -> Result<Self, AppError> {
        Self::ALL
            .into_iter()
            .find(|k| k.as_str() == raw)
            .ok_or_else(|| {
                AppError::validation(format!(
                    "unknown model kind {raw:?}; known kinds are {}",
                    Self::ALL.map(Self::as_str).join(", ")
                ))
            })
    }

    /// Stored/URL form.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Vision => "vision",
            Self::Image => "image",
            Self::Embedding => "embedding",
            Self::Moderation => "moderation",
            Self::Transcription => "transcription",
            Self::Speech => "speech",
        }
    }

    /// Display name.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Text => "Text generation",
            Self::Vision => "Vision",
            Self::Image => "Image generation",
            Self::Embedding => "Embeddings",
            Self::Moderation => "Moderation",
            Self::Transcription => "Transcription",
            Self::Speech => "Speech",
        }
    }

    /// What in the product uses a model of this kind today. Kinds with no
    /// consumer yet say so: registering them is preparation, not a feature.
    #[must_use]
    pub fn used_by(self) -> &'static str {
        match self {
            Self::Text => "Writing assistant, excerpt and SEO suggestions, theme builder, plugins",
            Self::Vision => "Alt text for uploaded images (Settings → AI features)",
            Self::Image => "Image generation from the editor (Settings → AI features)",
            Self::Embedding => {
                "Semantic search and the related-posts block (Settings → AI features)"
            }
            Self::Moderation => "Comment screening (Settings → AI features)",
            Self::Transcription => "Transcripts for audio and video files (Settings → AI features)",
            Self::Speech => "Read-aloud recordings of posts (Settings → AI features)",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_and_providers_round_trip_their_names() {
        for kind in ModelKind::ALL {
            assert_eq!(ModelKind::parse(kind.as_str()).unwrap(), kind);
        }
        for provider in ProviderId::ALL {
            assert_eq!(ProviderId::parse(provider.as_str()).unwrap(), provider);
        }
        assert!(ModelKind::parse("music").is_err());
        assert!(ProviderId::parse("google").is_err());
    }

    #[test]
    fn capability_matrix_matches_what_each_api_offers() {
        assert!(ProviderId::Anthropic.supports(ModelKind::Text));
        assert!(!ProviderId::Anthropic.supports(ModelKind::Embedding));
        assert!(ProviderId::OpenAi.supports(ModelKind::Speech));
        assert!(ProviderId::OpenRouter.supports(ModelKind::Embedding));
        assert!(!ProviderId::OpenRouter.supports(ModelKind::Transcription));
    }
}
