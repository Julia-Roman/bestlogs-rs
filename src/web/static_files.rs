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

/// Serves the embedded SvelteKit build: the exact asset if it exists, then
/// the route's prerendered page (`/contact` is `contact.html`), and otherwise
/// the SPA fallback with a 404 so the client-side router renders its error
/// page.
pub async fn fallback(uri: Uri) -> Response {
    let path = uri.path().trim_matches('/');
    let page = if path.is_empty() {
        "index.html".to_string()
    } else {
        format!("{path}.html")
    };

    if let Some(response) = serve(path).or_else(|| serve(&page)) {
        return response;
    }

    match serve("404.html") {
        Some(response) => (StatusCode::NOT_FOUND, response).into_response(),
        None => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}
