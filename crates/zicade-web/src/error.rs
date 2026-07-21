//! Typed web errors mapped to HTTP responses.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;
use zicade_config::ConfigError;

/// Errors surfaced by the config mutation endpoint.
#[derive(Debug, thiserror::Error)]
pub(crate) enum WebError {
    /// The submitted config failed to parse or validate.
    #[error(transparent)]
    Config(#[from] ConfigError),
}

impl IntoResponse for WebError {
    fn into_response(self) -> Response {
        let (status, message) = match &self {
            // I/O failures persisting a valid config are server-side.
            WebError::Config(ConfigError::Io { .. }) => {
                (StatusCode::INTERNAL_SERVER_ERROR, self.to_string())
            }
            // Parse / validation failures are client input errors.
            WebError::Config(_) => (StatusCode::BAD_REQUEST, self.to_string()),
        };
        (status, Json(json!({ "error": message }))).into_response()
    }
}
