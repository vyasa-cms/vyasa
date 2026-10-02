//! Typed site options: identity, formatting, permalinks, custom CSS.

mod service;

pub use service::{
    brand_kit_prompt_of, sanitize_custom_css, validate_brand_kit, CustomCss, OptionsService,
    PermalinkPattern, SiteIdentity, BRAND_KIT_FIELDS, FULL_ADMINISTRATOR_OPTION_KEYS,
    PUBLIC_OPTION_KEYS, SITE_OPTION_KEYS,
};
