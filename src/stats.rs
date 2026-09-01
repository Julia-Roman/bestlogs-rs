use std::sync::{Arc, LazyLock};
use std::time::Duration;

use axum::http::HeaderMap;
use serde_json::json;
use tokio::sync::Semaphore;
use tracing::{error, warn};

use crate::state::AppState;

/// Analytics posts get their own deadline. Every other outbound request in
/// this process sets one; without it these detached tasks inherit reqwest's
/// default of *no* overall timeout, so a hung Umami endpoint parks them —
/// each holding an `Arc<AppState>`, a JSON payload and a connection — for as
/// long as the TCP connection survives.
const STATS_TIMEOUT: Duration = Duration::from_secs(5);

/// Ceiling on stats posts in flight at once. One task is spawned per
/// incoming request, so if Umami is slower than our request rate the backlog
/// grows without bound and the queue itself becomes the leak. Analytics is
/// the least important thing this service does: past this point events are
/// dropped rather than queued.
const MAX_INFLIGHT_STATS: usize = 64;

static STATS_SLOTS: LazyLock<Arc<Semaphore>> =
    LazyLock::new(|| Arc::new(Semaphore::new(MAX_INFLIGHT_STATS)));

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

    let Ok(permit) = STATS_SLOTS.clone().try_acquire_owned() else {
        warn!("[Umami] Dropping '{name}' event: {MAX_INFLIGHT_STATS} posts already in flight");
        return;
    };

    tokio::spawn(async move {
        let _permit = permit;
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
            .timeout(STATS_TIMEOUT)
            .send()
            .await;

        if let Err(err) = result {
            error!("[Umami] Error sending '{name}' data: {err}");
        }
    });
}
