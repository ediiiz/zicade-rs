//! Config CRUD + status handlers.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use zicade_config::{Config, ValidationCtx};

use crate::error::WebError;
use crate::state::{AppState, StatusSnapshot};

/// `GET /api/config` — return the current in-memory config. Read-only and open
/// on loopback (not token-gated).
pub(crate) async fn get_config(State(state): State<AppState>) -> Json<Config> {
    Json(state.config_snapshot())
}

/// `PUT /api/config` — parse, validate, persist, and apply a new config.
///
/// The `String` extractor already rejects non-UTF-8 bodies with `400`. Parse
/// and validation failures map to `400`; a persistence I/O failure maps to
/// `500` (see [`WebError`]). Only reached after the token gate.
pub(crate) async fn put_config(
    State(state): State<AppState>,
    body: String,
) -> Result<StatusCode, WebError> {
    let config = zicade_config::from_json_str(&body)?;
    config.validate(&ValidationCtx::host())?;
    zicade_config::save_file(state.config_path(), &config)?;
    state.apply_config(config);
    Ok(StatusCode::OK)
}

/// `GET /api/status` — return the current status snapshot (read-only).
pub(crate) async fn get_status(State(state): State<AppState>) -> Json<StatusSnapshot> {
    Json(state.status_snapshot())
}
