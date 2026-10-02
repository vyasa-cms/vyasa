//! Registry operations: provider credentials, model CRUD, defaults, and the
//! resolution features call to find out which model to use for a job.

use vyasa_common::AppError;
use vyasa_db::repo::{AiModelRow, AiModelUpdate, AiModelsRepo, AiProviderRow, NewAiModel};

use super::kinds::{ModelKind, ProviderId};
use super::secret::{hint, KeyVault};

/// What the admin page shows about a provider. Never the key itself.
#[derive(Debug, Clone, serde::Serialize)]
#[allow(clippy::struct_excessive_bools)] // a status report: each flag is a separate fact
pub struct ProviderStatus {
    /// `anthropic` | `openai` | `openrouter`.
    pub provider: String,
    /// Display name.
    pub label: String,
    /// Whether a usable key exists (stored or from the environment).
    pub configured: bool,
    /// Where the key comes from: `stored`, `environment` or `none`.
    pub key_source: String,
    /// Last characters of the key, for recognition.
    pub key_hint: String,
    /// Whether the stored key is encrypted at rest.
    pub key_sealed: bool,
    /// Base URL override, empty for the default.
    pub base_url: String,
    /// The default base URL.
    pub default_base_url: String,
    /// Whether the provider may be used.
    pub enabled: bool,
    /// Kinds this provider's API can serve.
    pub supports: Vec<String>,
    /// False for a local OpenAI-compatible server.
    pub needs_key: bool,
    /// True when there is no sensible default base URL.
    pub needs_base_url: bool,
    /// A model to start with, per kind.
    pub recommended: std::collections::BTreeMap<String, super::kinds::Recommended>,
}

/// A registered model plus everything needed to call it.
#[derive(Debug, Clone)]
pub struct ResolvedModel {
    /// The registry row.
    pub row: AiModelRow,
    /// Which provider.
    pub provider: ProviderId,
    /// The decrypted API key.
    pub api_key: String,
    /// Effective base URL.
    pub base_url: String,
}

/// What it takes to register a model.
#[derive(Debug, Clone)]
pub struct NewModelSpec<'a> {
    /// Which provider.
    pub provider: ProviderId,
    /// Kind of job.
    pub kind: ModelKind,
    /// Provider model id.
    pub model: &'a str,
    /// Display name; empty means "use the id".
    pub label: &'a str,
    /// Per-model settings.
    pub settings: serde_json::Value,
    /// USD per million input tokens.
    pub input_cost_per_mtok: Option<f64>,
    /// USD per million output tokens.
    pub output_cost_per_mtok: Option<f64>,
}

/// The registry.
#[derive(Clone, Debug)]
pub struct AiModelsService {
    repo: AiModelsRepo,
    vault: KeyVault,
}

impl AiModelsService {
    /// Builds the service over its repo and the key vault.
    #[must_use]
    pub fn new(repo: AiModelsRepo, vault: KeyVault) -> Self {
        Self { repo, vault }
    }

    /// Whether stored keys are encrypted at rest.
    #[must_use]
    pub fn encrypting(&self) -> bool {
        self.vault.encrypting()
    }

    /// The environment fallback for a provider's key, if set.
    fn env_key(provider: ProviderId) -> Option<String> {
        std::env::var(provider.env_key())
            .ok()
            .filter(|v| !v.trim().is_empty())
    }

    /// Status of every provider the registry knows, configured or not.
    ///
    /// # Errors
    /// Database errors.
    pub async fn providers(&self) -> Result<Vec<ProviderStatus>, AppError> {
        let rows = self.repo.list_providers().await?;
        Ok(ProviderId::ALL
            .into_iter()
            .map(|id| {
                let row = rows.iter().find(|r| r.provider == id.as_str());
                self.status(id, row)
            })
            .collect())
    }

    fn status(&self, id: ProviderId, row: Option<&AiProviderRow>) -> ProviderStatus {
        let stored = row.map_or("", |r| r.api_key.as_str());
        let (configured, key_source, key_hint) = if !stored.is_empty() {
            let opened = self.vault.open(stored).ok();
            (
                true,
                "stored".to_owned(),
                opened
                    .as_deref()
                    .map_or_else(|| "(encrypted)".to_owned(), hint),
            )
        } else if let Some(env) = Self::env_key(id) {
            (true, "environment".to_owned(), hint(&env))
        } else if !id.needs_key() && row.is_some_and(|r| !r.base_url.is_empty()) {
            // A local server needs no key; the base URL is the whole setup.
            (true, "none".to_owned(), String::new())
        } else {
            (false, "none".to_owned(), String::new())
        };
        ProviderStatus {
            provider: id.as_str().to_owned(),
            label: id.label().to_owned(),
            configured,
            key_source,
            key_hint,
            key_sealed: KeyVault::is_sealed(stored),
            base_url: row.map(|r| r.base_url.clone()).unwrap_or_default(),
            default_base_url: id.default_base_url().to_owned(),
            enabled: row.is_none_or(|r| r.enabled),
            supports: ModelKind::ALL
                .into_iter()
                .filter(|k| id.supports(*k))
                .map(|k| k.as_str().to_owned())
                .collect(),
            needs_key: id.needs_key(),
            needs_base_url: id.needs_base_url(),
            recommended: ModelKind::ALL
                .into_iter()
                .filter_map(|k| id.recommended(k).map(|r| (k.as_str().to_owned(), r)))
                .collect(),
        }
    }

    /// Saves a provider's settings. `api_key = None` keeps the stored key;
    /// `Some("")` clears it (falling back to the environment, if any).
    ///
    /// # Errors
    /// Validation of the base URL; database errors.
    pub async fn save_provider(
        &self,
        provider: ProviderId,
        api_key: Option<&str>,
        base_url: &str,
        enabled: bool,
    ) -> Result<ProviderStatus, AppError> {
        let base_url = base_url.trim().trim_end_matches('/');
        if !base_url.is_empty()
            && !base_url.starts_with("https://")
            && !base_url.starts_with("http://")
        {
            return Err(AppError::validation(
                "base_url must start with http:// or https://",
            ));
        }
        let sealed = match api_key.map(str::trim) {
            None => None,
            Some("") => Some(String::new()),
            Some(key) => Some(self.vault.seal(key)?),
        };
        let row = self
            .repo
            .upsert_provider(provider.as_str(), sealed.as_deref(), base_url, enabled)
            .await?;
        Ok(self.status(provider, Some(&row)))
    }

    /// The credential and base URL to call a provider with, from the store
    /// first and the environment second.
    ///
    /// # Errors
    /// No key anywhere, or a sealed key that cannot be opened.
    pub async fn credentials(&self, provider: ProviderId) -> Result<(String, String), AppError> {
        let row = self.repo.get_provider(provider.as_str()).await?;
        if let Some(r) = &row {
            if !r.enabled {
                return Err(AppError::validation(format!(
                    "{} is disabled on the Models page",
                    provider.label()
                )));
            }
        }
        let base_url = row
            .as_ref()
            .map(|r| r.base_url.clone())
            .filter(|u| !u.is_empty())
            .unwrap_or_else(|| provider.default_base_url().to_owned());
        let stored = row.as_ref().map(|r| r.api_key.clone()).unwrap_or_default();
        let key = if stored.is_empty() {
            Self::env_key(provider).ok_or_else(|| {
                AppError::validation(format!(
                    "{} has no API key: add one on the Models page (or set {})",
                    provider.label(),
                    provider.env_key()
                ))
            })?
        } else {
            self.vault.open(&stored)?
        };
        Ok((key, base_url))
    }

    /// Every registered model.
    ///
    /// # Errors
    /// Database errors.
    pub async fn models(&self) -> Result<Vec<AiModelRow>, AppError> {
        self.repo.list_models().await
    }

    /// One model.
    ///
    /// # Errors
    /// Not found; database errors.
    pub async fn model(&self, id: i64) -> Result<AiModelRow, AppError> {
        self.repo.get_model(id).await
    }

    /// Registers a model after checking the provider can serve that kind.
    ///
    /// # Errors
    /// Validation (unsupported kind, empty id), conflicts, database errors.
    pub async fn register(&self, spec: NewModelSpec<'_>) -> Result<AiModelRow, AppError> {
        let NewModelSpec {
            provider,
            kind,
            model,
            label,
            settings,
            input_cost_per_mtok,
            output_cost_per_mtok,
        } = spec;
        let model = model.trim();
        if model.is_empty() {
            return Err(AppError::validation("model id is required"));
        }
        if model.len() > 200 || model.chars().any(char::is_whitespace) {
            return Err(AppError::validation(
                "model id must be a single token of at most 200 characters",
            ));
        }
        if !provider.supports(kind) {
            return Err(AppError::validation(format!(
                "{} does not offer {} models",
                provider.label(),
                kind.label().to_lowercase()
            )));
        }
        if !settings.is_object() {
            return Err(AppError::validation("settings must be an object"));
        }
        // The provider row must exist for the foreign key, even before a
        // key is stored (the environment may supply it).
        if self.repo.get_provider(provider.as_str()).await?.is_none() {
            self.repo
                .upsert_provider(provider.as_str(), None, "", true)
                .await?;
        }
        let label = if label.trim().is_empty() {
            model
        } else {
            label.trim()
        };
        self.repo
            .insert_model(&NewAiModel {
                provider: provider.as_str(),
                model,
                kind: kind.as_str(),
                label,
                settings,
                input_cost_per_mtok,
                output_cost_per_mtok,
            })
            .await
    }

    /// Sets the fallback order within a kind.
    ///
    /// # Errors
    /// Database errors.
    pub async fn reorder(&self, kind: ModelKind, ids: &[i64]) -> Result<(), AppError> {
        self.repo.set_order(kind.as_str(), ids).await
    }

    /// Month-to-date calls and spend per (provider, model).
    ///
    /// # Errors
    /// Database errors.
    pub async fn month_spend_by_model(&self) -> Result<Vec<(String, String, i64, f64)>, AppError> {
        self.repo.month_spend_by_model().await
    }

    /// Edits a model.
    ///
    /// # Errors
    /// Not found; database errors.
    pub async fn update(
        &self,
        id: i64,
        update: &AiModelUpdate<'_>,
    ) -> Result<AiModelRow, AppError> {
        if let Some(settings) = &update.settings {
            if !settings.is_object() {
                return Err(AppError::validation("settings must be an object"));
            }
        }
        self.repo.update_model(id, update).await
    }

    /// Makes a model the default of its kind.
    ///
    /// # Errors
    /// Not found; database errors.
    pub async fn set_default(&self, id: i64) -> Result<AiModelRow, AppError> {
        self.repo.set_default(id).await
    }

    /// Records a probe outcome.
    ///
    /// # Errors
    /// Database errors.
    pub async fn record_probe(&self, id: i64, ok: bool, detail: &str) -> Result<(), AppError> {
        self.repo.record_probe(id, ok, detail).await
    }

    /// Removes a model.
    ///
    /// # Errors
    /// Database errors.
    pub async fn remove(&self, id: i64) -> Result<bool, AppError> {
        self.repo.delete_model(id).await
    }

    /// Resolves one registered model with its credentials.
    ///
    /// # Errors
    /// Not found; credential problems.
    pub async fn resolve(&self, id: i64) -> Result<ResolvedModel, AppError> {
        let row = self.repo.get_model(id).await?;
        self.resolve_row(row).await
    }

    async fn resolve_row(&self, row: AiModelRow) -> Result<ResolvedModel, AppError> {
        let provider = ProviderId::parse(&row.provider)?;
        let (api_key, base_url) = self.credentials(provider).await?;
        Ok(ResolvedModel {
            row,
            provider,
            api_key,
            base_url,
        })
    }

    /// The models to try for a kind of job, default first, each with its
    /// credentials. Models whose provider has no usable key are skipped
    /// rather than failing the whole chain — the operator sees why on the
    /// Models page.
    ///
    /// # Errors
    /// Database errors.
    pub async fn chain(&self, kind: ModelKind) -> Result<Vec<ResolvedModel>, AppError> {
        let rows = self.repo.list_enabled_by_kind(kind.as_str()).await?;
        let mut out = Vec::new();
        for row in rows {
            match self.resolve_row(row).await {
                Ok(resolved) => out.push(resolved),
                Err(err) => tracing::warn!(kind = kind.as_str(), "skipping model: {err}"),
            }
        }
        Ok(out)
    }
}
