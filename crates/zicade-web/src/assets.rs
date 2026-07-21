//! Embedded static assets (the htmx-free UI) served from the binary.

use axum::extract::Path;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use rust_embed::Embed;

/// The UI files, embedded at compile time from `crates/zicade-web/assets/`.
#[derive(Embed)]
#[folder = "assets/"]
struct Assets;

/// `GET /` — serve the embedded `index.html`.
pub(crate) async fn index() -> Response {
    serve("index.html")
}

/// `GET /assets/{*path}` — serve an embedded asset by relative path.
pub(crate) async fn asset(Path(path): Path<String>) -> Response {
    serve(&path)
}

/// Look up an embedded file and respond with a content-type guessed from its
/// extension, or 404.
fn serve(path: &str) -> Response {
    match Assets::get(path) {
        Some(file) => (
            [(header::CONTENT_TYPE, mime_for(path))],
            file.data.into_owned(),
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// Minimal extension → content-type map for the assets we actually embed.
fn mime_for(path: &str) -> &'static str {
    match path.rsplit('.').next() {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json",
        Some("svg") => "image/svg+xml",
        _ => "application/octet-stream",
    }
}
