//! Canonical post addresses, read from the site's current settings.
use crate::state::AppState;
use vyasa_core::options::PermalinkPattern;
use vyasa_db::content_models::PostRow;

pub async fn pattern(state: &AppState) -> PermalinkPattern {
    state
        .options_service
        .permalink_pattern()
        .await
        .unwrap_or_else(|_| PermalinkPattern(PermalinkPattern::DEFAULT.into()))
}

pub async fn path(state: &AppState, post: &PostRow) -> String {
    pattern(state).await.path_for(post)
}
