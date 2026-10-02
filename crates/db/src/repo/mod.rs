//! Repositories: data access with sqlx. Domain rules live in
//! `vyasa-core`; these types only move rows.

pub mod ai_data;
pub mod ai_models;
pub mod api_keys;
pub mod audience;
pub mod audit;
pub mod comments;
pub mod content_fields;
pub mod content_types;
pub mod media;
pub mod menus;
pub mod options;
pub mod plugins;
pub mod posts;
pub mod revisions;
pub mod roles;
pub mod sessions;
pub mod terms;
pub mod theme_drafts;
pub mod themes;
pub mod tokens;
pub mod translations;
pub mod users;
pub mod webhooks;

pub use ai_data::{AiDataRepo, EmbeddingRow, PostAudioRow};
pub use ai_logs::AiLogRepo;
pub use ai_models::{AiModelRow, AiModelUpdate, AiModelsRepo, AiProviderRow, NewAiModel};
pub use api_keys::ApiKeysRepo;
pub use audience::{AudienceRepo, FormSubmissionRow, SubscriberRow, ViewRow};
pub use audit::{AuditRepo, AuditRow};
pub use comments::{CommentsRepo, NewComment};
pub use content_fields::{ContentFieldRow, ContentFieldUpdate, ContentFieldsRepo, NewContentField};
pub use content_types::{
    ContentTypeRow, ContentTypeUpdate, ContentTypesRepo, NewContentType, TypeDeletion,
};
pub use media::{MediaFilter, MediaRepo, MediaStats, NewMedia};
pub use menus::{MenuItemRow, MenuRow, MenusRepo, NewMenuItem};
pub use options::OptionsRepo;
pub use plugins::{PluginAuditRow, PluginRow, PluginsRepo};
pub use posts::{BoundQuery, NewPost, PostFilter, PostSort, PostUpdate, PostsRepo};
pub use revisions::{NewRevision, RevisionsRepo};
pub use roles::{NewRole, RoleUpdate, RolesRepo};
pub use sessions::SessionsRepo;
pub use terms::{NewTerm, TermUpdate, TermWithCount, TermsRepo};
pub use theme_drafts::{
    DraftDocuments, ThemeDraftMessageRow, ThemeDraftRevisionRow, ThemeDraftRow, ThemeDraftsRepo,
};
pub use themes::{ThemeFileInput, ThemeFileMeta, ThemeFileRow, ThemesRepo};
pub use tokens::TokensRepo;
pub use translations::{AlternateRow, TranslationRow, TranslationsRepo};
pub use users::{ConfirmPassword, NewUser, ResetRedeemed, UnconfirmedInsertError, UsersRepo};
pub use webhooks::{DeliveryRow, LastDelivery, WebhookRow, WebhooksRepo};

/// AI usage log repository (phase 37).
pub mod ai_logs;
