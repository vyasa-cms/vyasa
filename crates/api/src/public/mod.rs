//! Public site: theme-rendered pages for real traffic.
//!
//! Permalink decision (recorded per PHASE-26): posts live at
//! `/post/{slug}`, pages at `/{slug}`; archives at `/category/{slug}` and
//! `/tag/{slug}` with `/page/{n}` suffixes for pagination. All public GETs
//! go through the render cache (phase 23) and emit strong ETags.

pub mod cacheable;
pub mod comment_form;
mod errors;
pub mod form_submit;
pub mod health;
pub mod page_meta;
pub mod plugin_routes;
pub mod routes;
mod seo;

use axum::routing::{get, post};
use axum::Router;

use crate::state::AppState;

/// Builds the public router (mounted at the root, before /api).
pub fn router(state: &AppState) -> Router<AppState> {
    Router::new()
        .route("/", get(routes::home))
        .route("/post/{slug}", get(routes::single_post))
        .route("/post/{slug}/password", post(routes::unlock_post))
        // Every kind of entry unlocks here, by id; the per-slug route above
        // is kept for forms rendered before it existed.
        .route("/unlock", post(routes::unlock))
        // Pages claim the root namespace.
        .route("/{slug}", get(routes::single_page))
        // Registered post types: `/{type}/{slug}`. Static prefixes above
        // (`/post`, `/category`, `/tag`, `/archive`) still win, because
        // the router prefers a literal segment to a parameter.
        .route("/{type}/{slug}", get(routes::custom_single))
        .route("/archive/{year}/{month}", get(routes::date_archive))
        .route("/category/{slug}", get(routes::category_archive))
        .route("/tag/{slug}", get(routes::tag_archive))
        .route("/author/{username}", get(routes::author_archive))
        .route("/search", get(routes::search))
        .route("/comment", post(comment_form::submit))
        .route("/form", post(form_submit::submit))
        .route("/newsletter/confirm", get(form_submit::confirm))
        .route("/newsletter/unsubscribe", get(form_submit::unsubscribe))
        .route("/preview/{post_id}", get(routes::preview))
        .route(
            "/api/v1/plugin/{name}/{*rest}",
            axum::routing::any(plugin_routes::plugin_route),
        )
        .route("/plugin-assets/{file}", get(plugin_routes::plugin_asset))
        .route("/theme-assets/{*file}", get(routes::theme_asset))
        .route(
            "/preview/theme/{theme_id}",
            get(crate::public::routes::preview_theme),
        )
        .route("/favicon.ico", get(crate::public::routes::favicon))
        .route("/logo", get(crate::public::routes::logo))
        .route("/brand/mark.svg", get(crate::public::routes::brand_mark))
        .route("/custom.css", get(crate::public::routes::custom_css_route))
        .route("/feed.xml", get(crate::feeds::rss_route))
        .route("/atom.xml", get(crate::feeds::atom_route))
        .route("/sitemap.xml", get(seo::sitemap))
        .route("/robots.txt", get(seo::robots))
        // Liveness and readiness for whatever runs the container.
        .route("/healthz", get(health::healthz))
        .route("/readyz", get(health::readyz))
        .route("/llms.txt", get(seo::llms_txt))
        .route("/indexnow.txt", get(seo::indexnow_key))
        // A marketplace this site hosts. Both 404 unless the operator
        // has put files in `registry_dir`.
        .route("/registry/index.json", get(crate::registry::host::index))
        .route(
            "/registry/packages/{file}",
            get(crate::registry::host::package),
        )
        .fallback(axum::routing::any(errors::themed_fallback))
        // A 404 from any public route — including `/post/{slug}`, whose
        // handler answers before the fallback ever runs — is answered
        // with the redirect written when that address changed.
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            errors::redirect_on_not_found,
        ))
}
