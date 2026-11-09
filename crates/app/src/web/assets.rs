// SPDX-License-Identifier: AGPL-3.0-only
//! Embedded web UI (Vite build output in `web/dist`). Release builds embed
//! the files; nothing is loaded from a CDN.

use axum::http::{HeaderValue, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use rust_embed::Embed;

#[derive(Embed)]
#[folder = "../../web/dist"]
#[allow_missing = true]
struct Assets;

fn mime(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "ico" => "image/x-icon",
        "webp" => "image/webp",
        "woff2" => "font/woff2",
        "json" | "webmanifest" => "application/json",
        "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

pub async fn serve(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    if path.starts_with("api/") {
        return (
            StatusCode::NOT_FOUND,
            axum::Json(serde_json::json!({"error": "not found"})),
        )
            .into_response();
    }
    // Unknown paths fall back to index.html (client-side routing).
    let (file, name) = match Assets::get(path).filter(|_| !path.is_empty()) {
        Some(f) => (f, path.to_owned()),
        None => match Assets::get("index.html") {
            Some(f) => (f, "index.html".to_owned()),
            None => {
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    "Web UI not built. Run `npm ci && npm run build` in web/.",
                )
                    .into_response();
            }
        },
    };
    let mut resp = file.data.into_owned().into_response();
    let h = resp.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static(mime(&name)));
    // Vite puts content hashes in asset file names: cache those forever.
    let cache = if name.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    resp
}
