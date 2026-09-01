use std::sync::Arc;

use axum::http::HeaderMap;
use serde_json::json;
use tracing::error;

use crate::state::AppState;

/// Fire-and-forget port of `sendStats`: posts a page/API-usage event to
/// Umami. Spawned so it never delays the response it's attached to.
pub fn spawn_stats(
    state: Arc<AppState>,
    headers: &HeaderMap,
    url: &str,
    name: &str,
    data: serde_json::Value,
) {
    let Some(umami) = state.config.umami_stats.clone() else {
        return;
    };
    if umami.id.is_empty() || umami.token.is_empty() {
        return;
    }

    let hostname = headers
        .get(axum::http::header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let language = headers
        .get(axum::http::header::ACCEPT_LANGUAGE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let referrer = headers
        .get(axum::http::header::REFERER)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
        .or_else(|| state.config.instance.url.clone())
        .unwrap_or_default();
    let url = url.to_string();
    let name = name.to_string();

    tokio::spawn(async move {
        let payload = json!({
            "payload": {
                "hostname": hostname,
                "language": language,
                "referrer": referrer,
                "url": url,
                "website": umami.id,
                "name": name,
                "data": data,
            },
            "type": "event",
        });

        let result = state
            .http
            .post(format!("{}/api/send", umami.url))
            .bearer_auth(&umami.token)
            .json(&payload)
            .send()
            .await;

        if let Err(err) = result {
            error!("[Umami] Error sending '{name}' data: {err}");
        }
    });
}
