//! Provider catalogues and connectivity probes for the model registry.
//!
//! A catalogue is what a provider says the key can use, with the kinds of
//! job each entry looks fit for. A probe is the cheapest real call that
//! proves a registered model works: a tiny completion for text, a short
//! embedding for embeddings, and a catalogue lookup for everything else —
//! generating an image to prove an image model exists costs real money.

use serde_json::{json, Value};
use vyasa_common::AppError;
use vyasa_core::ai_models::{ModelKind, ProviderId};

use crate::provider::{LlmProvider, PromptSpec, ProviderError};
use crate::{AnthropicProvider, OpenAiProvider};

/// One model a provider offers.
#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
pub struct CatalogEntry {
    /// The provider's model id.
    pub id: String,
    /// Display name where the provider gives one.
    pub name: String,
    /// Kinds this model looks fit for, by the provider's metadata or the id.
    pub kinds: Vec<&'static str>,
}

/// Kinds an OpenAI model id is fit for, from its name. OpenAI's catalogue
/// carries no modality data, so this is a naming convention, not a fact.
#[must_use]
pub fn infer_openai_kinds(id: &str) -> Vec<&'static str> {
    let lower = id.to_ascii_lowercase();
    if lower.contains("embedding") {
        return vec!["embedding"];
    }
    if lower.contains("moderation") {
        return vec!["moderation"];
    }
    if lower.contains("dall-e") || lower.contains("gpt-image") {
        return vec!["image"];
    }
    if lower.contains("whisper") || lower.contains("transcribe") {
        return vec!["transcription"];
    }
    if lower.contains("tts") {
        return vec!["speech"];
    }
    if lower.contains("realtime") || lower.contains("audio") {
        return vec![];
    }
    if lower.starts_with("gpt-") || lower.starts_with('o') || lower.starts_with("chatgpt") {
        return vec!["text", "vision"];
    }
    vec!["text"]
}

/// Kinds an OpenRouter entry is fit for, from its architecture block.
#[must_use]
pub fn infer_openrouter_kinds(entry: &Value) -> Vec<&'static str> {
    let arch = &entry["architecture"];
    let inputs: Vec<String> = arch["input_modalities"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let outputs: Vec<String> = arch["output_modalities"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let modality = arch["modality"].as_str().unwrap_or("");
    let id = entry["id"].as_str().unwrap_or("").to_ascii_lowercase();

    if id.contains("embed") {
        return vec!["embedding"];
    }
    let mut kinds = Vec::new();
    let text_out =
        outputs.iter().any(|o| o == "text") || modality.ends_with("->text") || outputs.is_empty();
    if text_out {
        kinds.push("text");
        if inputs.iter().any(|i| i == "image") || modality.starts_with("text+image") {
            kinds.push("vision");
        }
    }
    if outputs.iter().any(|o| o == "image") {
        kinds.push("image");
    }
    kinds
}

/// The catalogue for a provider, sorted by id.
///
/// # Errors
/// Provider transport/status problems, or a blank key.
pub async fn list(
    provider: ProviderId,
    key: &str,
    base_url: &str,
) -> Result<Vec<CatalogEntry>, AppError> {
    let external = |e: ProviderError| crate::provider::external_error(provider.as_str(), &e);
    let mut out: Vec<CatalogEntry> = match provider {
        ProviderId::Anthropic => AnthropicProvider::new(key)?
            .with_base_url(base_url)
            .list_models()
            .await
            .map_err(external)?
            .iter()
            .filter_map(|m| {
                let id = m["id"].as_str()?;
                Some(CatalogEntry {
                    id: id.to_owned(),
                    name: m["display_name"].as_str().unwrap_or(id).to_owned(),
                    kinds: vec!["text", "vision"],
                })
            })
            .collect(),
        ProviderId::OpenAi => OpenAiProvider::new(key)
            .map_err(AppError::validation)?
            .with_base_url(base_url)
            .list_models()
            .await
            .map_err(external)?
            .iter()
            .filter_map(|m| {
                let id = m["id"].as_str()?;
                Some(CatalogEntry {
                    id: id.to_owned(),
                    name: id.to_owned(),
                    kinds: infer_openai_kinds(id),
                })
            })
            .collect(),
        ProviderId::OpenRouter => OpenAiProvider::openrouter(key)
            .map_err(AppError::validation)?
            .with_base_url(base_url)
            .list_models()
            .await
            .map_err(external)?
            .iter()
            .filter_map(|m| {
                let id = m["id"].as_str()?;
                Some(CatalogEntry {
                    id: id.to_owned(),
                    name: m["name"].as_str().unwrap_or(id).to_owned(),
                    kinds: infer_openrouter_kinds(m),
                })
            })
            .collect(),
        ProviderId::Gemini | ProviderId::Mistral | ProviderId::Custom => {
            compatible(provider, key, base_url)?
                .list_models()
                .await
                .map_err(external)?
                .iter()
                .filter_map(|m| {
                    let id = m["id"].as_str()?;
                    Some(CatalogEntry {
                        id: id.to_owned(),
                        name: m["name"]
                            .as_str()
                            .or_else(|| m["display_name"].as_str())
                            .unwrap_or(id)
                            .to_owned(),
                        kinds: infer_compatible_kinds(id),
                    })
                })
                .collect()
        }
    };
    out.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(out)
}

/// Kinds a model id from an OpenAI-compatible server probably serves,
/// by name: the compatible endpoints list ids without capabilities.
#[must_use]
pub fn infer_compatible_kinds(id: &str) -> Vec<&'static str> {
    let lower = id.to_ascii_lowercase();
    if lower.contains("embed") {
        vec!["embedding"]
    } else if lower.contains("moderation") {
        vec!["moderation"]
    } else if lower.contains("image") || lower.contains("imagen") {
        vec!["image"]
    } else if lower.contains("tts") {
        vec!["speech"]
    } else if lower.contains("whisper") || lower.contains("transcribe") {
        vec!["transcription"]
    } else {
        vec!["text", "vision"]
    }
}

/// An OpenAI-protocol client for every provider that speaks it. The base
/// URL falls back to the provider's default; a custom server must give one.
///
/// # Errors
/// Blank key where one is required; missing base URL for a custom server.
pub fn compatible(
    provider: ProviderId,
    key: &str,
    base_url: &str,
) -> Result<OpenAiProvider, AppError> {
    let url = if base_url.trim().is_empty() {
        provider.default_base_url()
    } else {
        base_url
    };
    let client = match provider {
        ProviderId::OpenAi => OpenAiProvider::new(key),
        ProviderId::OpenRouter => OpenAiProvider::openrouter(key),
        ProviderId::Gemini => OpenAiProvider::compatible(key, "gemini", url),
        ProviderId::Mistral => OpenAiProvider::compatible(key, "mistral", url),
        ProviderId::Custom => {
            if base_url.trim().is_empty() {
                return Err(AppError::validation(
                    "an OpenAI-compatible server needs its base URL, e.g. http://127.0.0.1:11434/v1",
                ));
            }
            OpenAiProvider::compatible(key, "custom", url)
        }
        ProviderId::Anthropic => {
            return Err(AppError::validation(
                "Anthropic does not speak the OpenAI protocol",
            ));
        }
    }
    .map_err(AppError::validation)?;
    Ok(client.with_base_url(url))
}

/// Builds the chat client for a provider, honouring the model's settings.
///
/// # Errors
/// Blank key.
pub fn chat_provider(
    provider: ProviderId,
    key: &str,
    base_url: &str,
    settings: &Value,
) -> Result<std::sync::Arc<dyn LlmProvider>, AppError> {
    Ok(match provider {
        ProviderId::Anthropic => std::sync::Arc::new(
            AnthropicProvider::new(key)?
                .with_base_url(base_url)
                .with_settings(settings),
        ),
        _ => std::sync::Arc::new(compatible(provider, key, base_url)?.with_settings(settings)),
    })
}

/// A 1x1 transparent PNG: the smallest picture a vision model can be shown.
const PROBE_PNG_B64: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkYAAAAAYAAjCB0C8AAAAASUVORK5CYII=";

/// Proves a registered model works, as cheaply as its kind allows. Returns
/// a one-line description of what was checked.
///
/// # Errors
/// Whatever the provider reported, as [`AppError::External`].
pub async fn probe(
    kind: ModelKind,
    provider: ProviderId,
    key: &str,
    base_url: &str,
    model: &str,
) -> Result<String, AppError> {
    let external = |e: ProviderError| crate::provider::external_error(provider.as_str(), &e);
    match kind {
        ModelKind::Text | ModelKind::Vision => {
            let client = chat_provider(provider, key, base_url, &json!({}))?;
            let attachments = if kind == ModelKind::Vision {
                vec![crate::provider::Attachment {
                    mime: String::from("image/png"),
                    data_b64: String::from(PROBE_PNG_B64),
                }]
            } else {
                Vec::new()
            };
            let spec = PromptSpec {
                attachments,
                purpose: String::from("model-probe"),
                model: model.to_owned(),
                system: String::from("You confirm connectivity."),
                user: if kind == ModelKind::Vision {
                    String::from("An image is attached. Reply with {\"ok\": true}.")
                } else {
                    String::from("Reply with {\"ok\": true}.")
                },
                output_schema: json!({
                    "type": "object",
                    "properties": {"ok": {"type": "boolean"}},
                    "required": ["ok"],
                    "additionalProperties": false
                }),
            };
            let completion = client.complete(&spec).await.map_err(external)?;
            Ok(format!(
                "{} ({} in, {} out tokens)",
                if kind == ModelKind::Vision {
                    "Saw the test image and replied"
                } else {
                    "Replied"
                },
                completion.usage.prompt_tokens,
                completion.usage.completion_tokens
            ))
        }
        ModelKind::Embedding => {
            let client = compatible(provider, key, base_url)?;
            let dims = client.embed_probe(model, "vyasa").await.map_err(external)?;
            Ok(format!("Embedded a test string ({dims} dimensions)"))
        }
        ModelKind::Moderation => {
            let client = compatible(provider, key, base_url)?;
            let verdict = client
                .moderate(model, "Hello there.")
                .await
                .map_err(external)?;
            Ok(format!(
                "Screened a test sentence ({})",
                if verdict.flagged {
                    "flagged, oddly"
                } else {
                    "clean"
                }
            ))
        }
        ModelKind::Speech => {
            let client = compatible(provider, key, base_url)?;
            let bytes = client
                .speak(model, "Vyasa", "alloy")
                .await
                .map_err(external)?;
            Ok(format!(
                "Synthesised one word ({} bytes of audio)",
                bytes.len()
            ))
        }
        ModelKind::Image | ModelKind::Transcription => {
            // Generating an image costs real money and transcription needs
            // a clip; the honest cheap check is that the id exists.
            let catalog = list(provider, key, base_url).await?;
            if catalog.iter().any(|entry| entry.id == model) {
                Ok(format!(
                    "Listed in the {} catalogue; not exercised, to avoid the charge",
                    provider.label()
                ))
            } else {
                Err(AppError::validation(format!(
                    "{model} is not in the {} catalogue for this key",
                    provider.label()
                )))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openai_ids_map_to_kinds_by_name() {
        assert_eq!(infer_openai_kinds("gpt-4o-mini"), vec!["text", "vision"]);
        assert_eq!(
            infer_openai_kinds("text-embedding-3-small"),
            vec!["embedding"]
        );
        assert_eq!(infer_openai_kinds("dall-e-3"), vec!["image"]);
        assert_eq!(infer_openai_kinds("gpt-image-1"), vec!["image"]);
        assert_eq!(infer_openai_kinds("whisper-1"), vec!["transcription"]);
        assert_eq!(infer_openai_kinds("tts-1-hd"), vec!["speech"]);
        assert_eq!(
            infer_openai_kinds("omni-moderation-latest"),
            vec!["moderation"]
        );
        assert_eq!(infer_openai_kinds("o3-mini"), vec!["text", "vision"]);
    }

    #[test]
    fn openrouter_entries_map_to_kinds_by_modality() {
        let vision = json!({"id": "anthropic/claude-sonnet-4", "architecture": {
            "modality": "text+image->text", "input_modalities": ["text", "image"], "output_modalities": ["text"]}});
        assert_eq!(infer_openrouter_kinds(&vision), vec!["text", "vision"]);
        let image = json!({"id": "google/gemini-2.5-flash-image", "architecture": {
            "input_modalities": ["text", "image"], "output_modalities": ["image", "text"]}});
        assert_eq!(
            infer_openrouter_kinds(&image),
            vec!["text", "vision", "image"]
        );
        let embed = json!({"id": "openai/text-embedding-3-large", "architecture": {}});
        assert_eq!(infer_openrouter_kinds(&embed), vec!["embedding"]);
        let bare =
            json!({"id": "mistralai/mistral-7b", "architecture": {"modality": "text->text"}});
        assert_eq!(infer_openrouter_kinds(&bare), vec!["text"]);
    }
}
