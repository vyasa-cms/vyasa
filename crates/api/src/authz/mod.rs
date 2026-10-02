//! Route-level authorization: every `/api/v1` route declares the access
//! it needs where it is registered, and the check runs before its handler.
//!
//! [`Guarded`] is the only way REST routes are registered. There is no
//! method that takes a handler without an [`Access`], so a route cannot be
//! added without deciding who may call it, and [`Guarded::finish`] returns
//! the table of those decisions for the matrix test and the published
//! access table.

use std::future::Future;
use std::pin::Pin;

use axum::extract::{FromRequestParts, Request};
use axum::handler::Handler;
use axum::http::request::Parts;
use axum::http::Method;
use axum::response::{IntoResponse, Response};
use axum::routing::{MethodFilter, MethodRouter};
use axum::Router;
use vyasa_core::user::{cap_name, Capability};

use crate::middleware::Principal;
use crate::state::AppState;

#[cfg(test)]
mod matrix_tests;
#[cfg(test)]
pub(crate) mod tests;

/// Who may call a route.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    /// No authentication: sign-in, setup, public submissions.
    Public,
    /// Any signed-in principal.
    Authenticated,
    /// A principal holding this capability.
    Cap(Capability),
    /// A principal holding at least one of these.
    AnyOf(&'static [Capability]),
}

impl Access {
    /// Stable text for the access table.
    // Read only by the access table (docs/ROUTE-ACCESS.md) test.
    #[cfg_attr(not(test), allow(dead_code))]
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Access::Public => "public".to_owned(),
            Access::Authenticated => "authenticated".to_owned(),
            Access::Cap(cap) => cap_name(*cap).to_owned(),
            Access::AnyOf(caps) => caps
                .iter()
                .map(|cap| cap_name(*cap))
                .collect::<Vec<_>>()
                .join(" or "),
        }
    }
}

/// One declared decision: this method on this path needs this access.
// Read only by the role matrix and the access table, which are tests.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Clone, Debug)]
pub struct RouteRule {
    /// HTTP method.
    pub method: Method,
    /// Path as registered, relative to `/api/v1`.
    pub path: &'static str,
    /// Required access.
    pub access: Access,
}

/// A router whose every route carries a declared [`Access`].
#[derive(Default)]
pub struct Guarded {
    router: Router<AppState>,
    rules: Vec<RouteRule>,
}

/// The methods of one path, each with its access.
pub struct PathRoutes {
    path: &'static str,
    methods: MethodRouter<AppState>,
    rules: Vec<RouteRule>,
}

impl Guarded {
    /// An empty router.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers the methods of `path`.
    #[must_use]
    pub fn at(self, path: &'static str, build: impl FnOnce(PathRoutes) -> PathRoutes) -> Self {
        self.at_layered(path, build, |methods| methods)
    }

    /// Like [`Guarded::at`], for a path whose methods carry a layer of
    /// their own (a body limit). The layer wraps the guarded handlers.
    #[must_use]
    pub fn at_layered(
        mut self,
        path: &'static str,
        build: impl FnOnce(PathRoutes) -> PathRoutes,
        layer: impl FnOnce(MethodRouter<AppState>) -> MethodRouter<AppState>,
    ) -> Self {
        let routes = build(PathRoutes {
            path,
            methods: MethodRouter::new(),
            rules: Vec::new(),
        });
        self.router = self.router.route(path, layer(routes.methods));
        self.rules.extend(routes.rules);
        self
    }

    /// Adds another guarded router's routes and rules.
    #[must_use]
    pub fn merge(mut self, other: Guarded) -> Self {
        self.router = self.router.merge(other.router);
        self.rules.extend(other.rules);
        self
    }

    /// The router and the table of what it enforces.
    pub fn finish(self) -> (Router<AppState>, Vec<RouteRule>) {
        (self.router, self.rules)
    }
}

impl PathRoutes {
    fn on<H, T>(mut self, filter: MethodFilter, method: Method, access: Access, handler: H) -> Self
    where
        H: Handler<T, AppState>,
        T: 'static,
    {
        self.rules.push(RouteRule {
            method,
            path: self.path,
            access,
        });
        self.methods = self.methods.on(filter, Guard { access, handler });
        self
    }

    /// `GET`.
    #[must_use]
    pub fn get<H, T>(self, access: Access, handler: H) -> Self
    where
        H: Handler<T, AppState>,
        T: 'static,
    {
        self.on(MethodFilter::GET, Method::GET, access, handler)
    }

    /// `POST`.
    #[must_use]
    pub fn post<H, T>(self, access: Access, handler: H) -> Self
    where
        H: Handler<T, AppState>,
        T: 'static,
    {
        self.on(MethodFilter::POST, Method::POST, access, handler)
    }

    /// `PUT`.
    #[must_use]
    pub fn put<H, T>(self, access: Access, handler: H) -> Self
    where
        H: Handler<T, AppState>,
        T: 'static,
    {
        self.on(MethodFilter::PUT, Method::PUT, access, handler)
    }

    /// `PATCH`.
    #[must_use]
    pub fn patch<H, T>(self, access: Access, handler: H) -> Self
    where
        H: Handler<T, AppState>,
        T: 'static,
    {
        self.on(MethodFilter::PATCH, Method::PATCH, access, handler)
    }

    /// `DELETE`.
    #[must_use]
    pub fn delete<H, T>(self, access: Access, handler: H) -> Self
    where
        H: Handler<T, AppState>,
        T: 'static,
    {
        self.on(MethodFilter::DELETE, Method::DELETE, access, handler)
    }
}

/// `handler` with `access` decided first. The wrapper is the handler axum
/// sees, so no layer ordering can run the inner one unguarded.
#[derive(Clone)]
struct Guard<H> {
    access: Access,
    handler: H,
}

impl<H, T> Handler<T, AppState> for Guard<H>
where
    H: Handler<T, AppState>,
    T: 'static,
{
    type Future = Pin<Box<dyn Future<Output = Response> + Send>>;

    fn call(self, request: Request, state: AppState) -> Self::Future {
        Box::pin(async move {
            let (mut parts, body) = request.into_parts();
            if let Err(rejection) = authorize(&mut parts, &state, self.access).await {
                return rejection;
            }
            self.handler
                .call(Request::from_parts(parts, body), state)
                .await
        })
    }
}

/// Decides `access` for this request, leaving the resolved principal in
/// the request's extensions for the extractors to reuse.
///
/// # Errors
/// The 401 or 403 response to send instead of calling the handler.
pub(crate) async fn authorize(
    parts: &mut Parts,
    state: &AppState,
    access: Access,
) -> Result<(), Response> {
    if access == Access::Public {
        return Ok(());
    }
    let principal = Principal::from_request_parts(parts, state)
        .await
        .map_err(IntoResponse::into_response)?;
    let allowed = match access {
        Access::Public | Access::Authenticated => Ok(()),
        Access::Cap(cap) => principal.ensure(cap),
        Access::AnyOf(caps) => caps
            .iter()
            .find_map(|cap| principal.ensure(*cap).ok())
            .ok_or_else(|| vyasa_common::AppError::forbidden("insufficient capability")),
    };
    allowed.map_err(|err| crate::error::ApiError::from(err).into_response())?;
    parts.extensions.insert(principal);
    Ok(())
}
