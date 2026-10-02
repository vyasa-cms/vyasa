//! Error → HTTP response mapping for the REST surface.
//!
//! Every handler returns `Result<T, ApiError>`; the single `IntoResponse`
//! impl guarantees uniform problem bodies with the machine error code
//! from [`vyasa_common::AppError::error_code`].

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;
use vyasa_common::AppError;

/// The API-wide error wrapper.
pub struct ApiError(pub AppError);

/// Error body returned by every failing endpoint.
#[derive(Serialize, utoipa::ToSchema)]
pub struct ApiErrorBody {
    /// Stable machine code, e.g. `unauthorized`.
    pub code: String,
    /// Human-readable message (safe for display).
    pub message: String,
}

impl From<AppError> for ApiError {
    fn from(err: AppError) -> Self {
        Self(err)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status =
            StatusCode::from_u16(self.0.http_status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        // Not an error to the client: the password step passed and a
        // second factor is wanted. The challenge rides along.
        if let AppError::MfaRequired { challenge } = &self.0 {
            let body = Json(serde_json::json!({
                "code": "mfa_required",
                "message": "a second factor is needed",
                "mfa_required": true,
                "challenge": challenge,
            }));
            return (StatusCode::ACCEPTED, body).into_response();
        }
        let body = Json(ApiErrorBody {
            code: self.0.error_code(),
            message: self.0.client_message(),
        });
        (status, body).into_response()
    }
}

/// Result alias for handlers.
pub type ApiResult<T> = Result<T, ApiError>;
