//! Router assembly and the token gate on mutating routes.

use axum::Router;
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, put};

use crate::state::AppState;
use crate::{assets, handlers, sse};

/// The header carrying the local UI token on mutating requests.
pub const TOKEN_HEADER: &str = "x-zicade-token";

/// Build the axum router.
///
/// Read-only endpoints (`GET`) are open on loopback. Mutating endpoints
/// (`PUT /api/config`) sit behind [`token_gate`], which requires a matching
/// `X-Zicade-Token` header. That custom-header requirement *is* the CSRF
/// defense: a cross-site `<form>` or image request cannot set an arbitrary
/// request header, so it can never satisfy the gate. The server is expected to
/// be bound to `127.0.0.1` only (see the app wiring / crate docs).
pub fn router(state: AppState) -> Router {
    let gated = Router::new()
        .route("/api/config", put(handlers::put_config))
        .route_layer(middleware::from_fn_with_state(state.clone(), token_gate));

    Router::new()
        .route("/api/config", get(handlers::get_config))
        .route("/api/status", get(handlers::get_status))
        .route("/events/logs", get(sse::sse_logs))
        .route("/", get(assets::index))
        .route("/assets/{*path}", get(assets::asset))
        .merge(gated)
        .with_state(state)
}

/// Reject any mutating request whose `X-Zicade-Token` header is missing or does
/// not match the configured token, with `401 Unauthorized`.
async fn token_gate(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let provided = req
        .headers()
        .get(TOKEN_HEADER)
        .and_then(|value| value.to_str().ok());
    if provided == Some(state.token()) {
        next.run(req).await
    } else {
        StatusCode::UNAUTHORIZED.into_response()
    }
}
