#![allow(clippy::all, clippy::pedantic, clippy::nursery, clippy::restriction)]
#![allow(clippy::unwrap_used, clippy::expect_used)]
#![allow(missing_docs)]
//! GraphQL surface for Vyasa.
//!
//! Schema lives only in `api`; core stays oblivious. Depth/complexity limits
//! are 8 / 1000 as per phase 17.

use async_graphql::dataloader::DataLoader;
use async_graphql::Schema;
use async_graphql_axum::{GraphQLProtocol, GraphQLRequest, GraphQLResponse, GraphQLWebSocket};
use axum::extract::{State, WebSocketUpgrade};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::{Html, IntoResponse};

use crate::middleware::api_key::ApiKeyPrincipal;
use crate::middleware::auth::CurrentUser;
use crate::state::AppState;

pub mod context;
pub mod loaders;
pub mod mutations;
pub mod schema;
pub mod subscriptions;
pub mod types;

#[cfg(test)]
mod tests;

use context::GqlContext;
use loaders::{AuthorLoader, MediaLoader, TermsForPostLoader};
use mutations::MutationRoot;
use schema::QueryRoot;
use subscriptions::SubscriptionRoot;

/// Full schema type.
pub type VyasaSchema = Schema<QueryRoot, MutationRoot, SubscriptionRoot>;

/// Builds the schema (depth 8, complexity 1000) bound to no pool — loaders are per-request.
#[must_use]
pub fn build_schema() -> VyasaSchema {
    Schema::build(QueryRoot, MutationRoot, SubscriptionRoot)
        .limit_depth(8)
        .limit_complexity(1000)
        .extension(FreshFieldDefinitions)
        .finish()
}

/// Gives every operation its own field-definitions cache
/// ([`crate::entry_fields::SharedDefinitions`]), over HTTP and over a
/// WebSocket alike: per operation, not per connection, so a long-lived
/// subscription sees a field added or deleted since it connected. A
/// request that already carries one (a test) keeps it.
struct FreshFieldDefinitions;

impl async_graphql::extensions::ExtensionFactory for FreshFieldDefinitions {
    fn create(&self) -> std::sync::Arc<dyn async_graphql::extensions::Extension> {
        std::sync::Arc::new(FreshFieldDefinitions)
    }
}

#[async_graphql::async_trait::async_trait]
impl async_graphql::extensions::Extension for FreshFieldDefinitions {
    async fn prepare_request(
        &self,
        ctx: &async_graphql::extensions::ExtensionContext<'_>,
        request: async_graphql::Request,
        next: async_graphql::extensions::NextPrepareRequest<'_>,
    ) -> async_graphql::ServerResult<async_graphql::Request> {
        next.run(ctx, with_fresh_definitions(request)).await
    }
}

/// `request` with a field-definitions cache of its own, unless it has one.
fn with_fresh_definitions(request: async_graphql::Request) -> async_graphql::Request {
    let key = std::any::TypeId::of::<std::sync::Arc<crate::entry_fields::SharedDefinitions>>();
    if request.data.contains_key(&key) {
        request
    } else {
        request.data(std::sync::Arc::new(
            crate::entry_fields::SharedDefinitions::default(),
        ))
    }
}

/// Axum handler for `POST /api/graphql` and `GET /api/graphql?query=...`.
///
/// A GET may only read. The session cookie is `SameSite=Lax`, which a
/// browser still sends on a cross-site top-level GET, and the CSRF check
/// covers unsafe methods only — so a mutation over GET was a link that
/// acted as whoever clicked it.
pub async fn graphql_handler(
    State(state): State<AppState>,
    method: Method,
    headers: HeaderMap,
    req: GraphQLRequest,
) -> axum::response::Response {
    let req = req.into_inner();
    if method == Method::GET && !is_read_only(&req.query) {
        return (
            StatusCode::METHOD_NOT_ALLOWED,
            [(axum::http::header::ALLOW, "POST")],
            "mutations and subscriptions must be sent with POST",
        )
            .into_response();
    }
    let principal = resolve_principal(&state, &headers).await;
    let ctx = GqlContext {
        state: state.clone(),
        principal,
    };
    let schema = build_schema();
    // Per-request loaders with batched pool.
    let author_loader = DataLoader::new(AuthorLoader::new(state.pool.clone()), tokio::spawn);
    let terms_loader = DataLoader::new(TermsForPostLoader::new(state.pool.clone()), tokio::spawn);
    let media_loader = DataLoader::new(MediaLoader::new(state.pool.clone()), tokio::spawn);

    let request = req
        .data(ctx)
        .data(author_loader)
        .data(terms_loader)
        .data(media_loader)
        .data(state.clone());
    let res = schema.execute(request).await;
    GraphQLResponse::from(res).into_response()
}

/// Whether every operation in `query` is a query. A document that does not
/// parse counts as not read-only; POST is where its error belongs.
fn is_read_only(query: &str) -> bool {
    use async_graphql::parser::types::{DocumentOperations, OperationType};
    let Ok(doc) = async_graphql::parser::parse_query(query) else {
        return false;
    };
    let is_query = |ty: OperationType| ty == OperationType::Query;
    match &doc.operations {
        DocumentOperations::Single(op) => is_query(op.node.ty),
        DocumentOperations::Multiple(ops) => ops.values().all(|op| is_query(op.node.ty)),
    }
}

/// Playground handler — gated to `debug` mode / debug builds.
pub async fn playground_handler(State(state): State<AppState>) -> impl IntoResponse {
    let enabled =
        state.config.debug || cfg!(debug_assertions) || std::env::var("VYASA_DEBUG").is_ok();
    if !enabled {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    }
    let html = async_graphql::http::playground_source(
        async_graphql::http::GraphQLPlaygroundConfig::new("/api/graphql"),
    );
    Html(html).into_response()
}

/// WebSocket handler for GraphQL subscriptions — `/api/graphql/ws`.
pub async fn graphql_ws_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
    protocol: GraphQLProtocol,
) -> impl IntoResponse {
    let principal = resolve_principal(&state, &headers).await;
    let ctx = GqlContext {
        state: state.clone(),
        principal,
    };
    let schema = build_schema();
    let author_loader = DataLoader::new(AuthorLoader::new(state.pool.clone()), tokio::spawn);
    let terms_loader = DataLoader::new(TermsForPostLoader::new(state.pool.clone()), tokio::spawn);
    let media_loader = DataLoader::new(MediaLoader::new(state.pool.clone()), tokio::spawn);
    let mut data = async_graphql::Data::default();
    data.insert(ctx);
    data.insert(author_loader);
    data.insert(terms_loader);
    data.insert(media_loader);
    data.insert(state.clone());
    ws.protocols(async_graphql::http::ALL_WEBSOCKET_PROTOCOLS)
        .on_upgrade(move |socket| {
            GraphQLWebSocket::new(socket, schema, protocol)
                .with_data(data)
                .serve()
        })
}

async fn resolve_principal(
    state: &AppState,
    headers: &HeaderMap,
) -> Option<crate::middleware::Principal> {
    // Try bearer first.
    if let Some(auth) = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
    {
        if !auth.is_empty() {
            // Hash and resolve via api_keys repo.
            let hash = ApiKeyPrincipal::hash_raw(auth);
            if let Ok((key, user)) = state.api_keys.resolve(&hash).await {
                return Some(crate::middleware::Principal::ApiKey(ApiKeyPrincipal {
                    key,
                    user,
                }));
            }
            // Invalid key → no principal (GraphQL will treat as unauth for public fields)
            return None;
        }
    }
    // Try cookie.
    if let Some(cookie) = headers
        .get(axum::http::header::COOKIE)
        .and_then(|v| v.to_str().ok())
    {
        for pair in cookie.split(';') {
            let pair = pair.trim();
            if let Some(value) = pair.strip_prefix(crate::middleware::SESSION_COOKIE) {
                let value = value.strip_prefix('=').unwrap_or("");
                if value.is_empty() {
                    continue;
                }
                if let Ok(session) = state.auth.resolve(value).await {
                    return Some(crate::middleware::Principal::Session(CurrentUser {
                        token: session.token,
                        user: session.user,
                    }));
                }
            }
        }
    }
    None
}
