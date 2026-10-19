//! Config CRUD + status handlers.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use zicade_config::{Config, ValidationCtx};

use crate::error::WebError;
use crate::state::{AppState, StatusSnapshot};

/// `GET /api/config` — return the current in-memory config with passwords
/// removed. Read-only and open on loopback (not token-gated), so it must not
/// hand out stored credentials.
pub(crate) async fn get_config(State(state): State<AppState>) -> Json<Config> {
    Json(state.config_snapshot().without_secrets())
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
    let mut config = zicade_config::from_json_str(&body)?;
    // The UI only ever saw the redacted config; keep passwords it left out.
    config.keep_secrets_from(&state.config_snapshot());
    config.validate(&ValidationCtx::host())?;
    zicade_config::save_file(state.config_path(), &config)?;
    state.apply_config(config);
    Ok(StatusCode::OK)
}

/// `GET /api/status` — return the current status snapshot (read-only).
pub(crate) async fn get_status(State(state): State<AppState>) -> Json<StatusSnapshot> {
    Json(state.status_snapshot())
}
