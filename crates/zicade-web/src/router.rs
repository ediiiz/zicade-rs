//! Router assembly and the token gate on mutating routes.

use axum::Router;
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::http::header::HOST;
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
        .route("/events/metrics", get(sse::sse_metrics))
        .route("/", get(assets::index))
        .route("/assets/{*path}", get(assets::asset))
        .merge(gated)
        .layer(middleware::from_fn(loopback_host_only))
        .with_state(state)
}

/// Reject requests whose `Host` names anything but loopback.
///
/// Binding to `127.0.0.1` stops other machines, but not a web page in the
/// user's browser: via DNS rebinding, `attacker.example` can resolve to
/// `127.0.0.1` and then read this API as same-origin. The browser still sends
/// `Host: attacker.example`, which is what this check catches. A request with
/// no `Host` header cannot come from a browser and is let through.
async fn loopback_host_only(req: Request, next: Next) -> Response {
    let allowed = req
        .headers()
        .get(HOST)
        .is_none_or(|value| value.to_str().is_ok_and(is_loopback_host));
    if allowed {
        next.run(req).await
    } else {
        StatusCode::FORBIDDEN.into_response()
    }
}

/// Whether a `Host` header value (`name[:port]`) names a loopback host.
fn is_loopback_host(host: &str) -> bool {
    let name = match host.strip_prefix('[') {
        Some(rest) => rest.split(']').next().unwrap_or(""),
        None => host.rsplit_once(':').map_or(host, |(name, _)| name),
    };
    name.eq_ignore_ascii_case("localhost")
        || name
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// Gate mutating requests. When `web.authRequired` is `false` (the default) the
/// request passes straight through — the loopback-only bind is the only control
/// needed and auth is normally unneeded. When it is `true`, a request whose
/// `X-Zicade-Token` header is missing or does not match the configured token is
/// rejected with `401 Unauthorized` (preserving the prior behavior, which also
/// serves as CSRF protection).
async fn token_gate(State(state): State<AppState>, req: Request, next: Next) -> Response {
    if !state.auth_required() {
        return next.run(req).await;
    }
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
