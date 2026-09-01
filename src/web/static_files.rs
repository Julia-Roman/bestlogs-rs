use axum::http::{StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "frontend/build/"]
struct Assets;

fn serve(path: &str) -> Option<Response> {
    let asset = Assets::get(path)?;
    let mime = asset.metadata.mimetype();
    Some(([(header::CONTENT_TYPE, mime.to_string())], asset.data).into_response())
}

/// Serves the embedded SvelteKit build: the exact asset if it exists,
/// otherwise `index.html` so the client-side router can take over (the
/// build uses adapter-static's SPA fallback mode).
pub async fn fallback(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');

    if let Some(response) = serve(path) {
        return response;
    }

    match serve("index.html") {
        Some(response) => response,
        None => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}
