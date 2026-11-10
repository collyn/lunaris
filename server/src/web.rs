//! The React web client, embedded into the binary at build time.
//!
//! Release builds carry `web/dist` inside the executable, so the server is a
//! single file. When a `web/dist` directory exists on disk (development) it is
//! served from there instead — see `main.rs`.

use axum::{
    body::Body,
    http::{header, StatusCode, Uri},
    response::{IntoResponse, Response},
};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "../web/dist"]
#[allow_missing = true]
struct WebAssets;

/// Whether the web client was embedded (web/dist existed when building).
pub fn is_embedded() -> bool {
    WebAssets::get("index.html").is_some()
}

/// Fallback handler: static files, with index.html for client-side routes.
pub async fn serve(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };

    if let Some(file) = WebAssets::get(path) {
        // Vite fingerprints everything under assets/, so it can be cached forever.
        let cache = if path.starts_with("assets/") {
            "public, max-age=31536000, immutable"
        } else {
            "no-cache"
        };
        return (
            [
                (header::CONTENT_TYPE, file.metadata.mimetype().to_string()),
                (header::CACHE_CONTROL, cache.to_string()),
            ],
            Body::from(file.data.into_owned()),
        )
            .into_response();
    }

    match WebAssets::get("index.html") {
        Some(index) => (
            [
                (header::CONTENT_TYPE, "text/html; charset=utf-8".to_string()),
                (header::CACHE_CONTROL, "no-cache".to_string()),
            ],
            Body::from(index.data.into_owned()),
        )
            .into_response(),
        None => (
            StatusCode::NOT_FOUND,
            "Web client not available: this build has no embedded web/dist.",
        )
            .into_response(),
    }
}
