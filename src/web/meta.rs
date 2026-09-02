use std::sync::Arc;

use axum::Json;
use axum::extract::{Query, State};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::json;

use crate::logs::search;
use crate::state::{AppState, RELOAD_INTERVAL_MS};
use crate::twitch::get_info;

pub async fn meta(State(state): State<Arc<AppState>>) -> Response {
    let umami = state
        .config
        .umami_stats
        .as_ref()
        .filter(|u| !u.id.is_empty())
        .map(|u| json!({ "url": u.url, "id": u.id }));

    Json(json!({
        "version": state.version,
        "commit": state.commit,
        "instances": state.config.justlogs_instances.keys().collect::<Vec<_>>(),
        "instance": state.config.instance,
        "umami": umami,
    }))
    .into_response()
}

pub async fn contact(State(state): State<Arc<AppState>>) -> Response {
    let creator = match get_info(&state, "ZonianMidian").await {
        Ok(info) => {
            json!({ "name": info.name, "login": info.login, "avatar": info.avatar, "id": info.id })
        }
        Err(_) => {
            return (
                axum::http::StatusCode::BAD_GATEWAY,
                "Failed to resolve creator info",
            )
                .into_response();
        }
    };

    let has_instance = state.config.instance.maintainer.is_some()
        || state.config.instance.message.is_some()
        || state.config.instance.country.is_some()
        || state.config.instance.city.is_some()
        || state.config.instance.flag.is_some()
        || state.config.instance.url.is_some();

    let maintainer = if has_instance {
        let mut merged = serde_json::to_value(&state.config.instance).unwrap_or_else(|_| json!({}));

        if let Some(maintainer_login) = &state.config.instance.maintainer
            && let Ok(info) = get_info(&state, maintainer_login).await
            && let serde_json::Value::Object(map) = &mut merged
        {
            map.insert("name".to_string(), json!(info.name));
            map.insert("login".to_string(), json!(info.login));
            map.insert("avatar".to_string(), json!(info.avatar));
            map.insert("id".to_string(), json!(info.id));
        }

        Some(merged)
    } else {
        None
    };

    Json(json!({ "creator": creator, "maintainer": maintainer })).into_response()
}

pub async fn status(State(state): State<Arc<AppState>>) -> Response {
    let instances: serde_json::Map<String, serde_json::Value> = state
        .config
        .justlogs_instances
        .iter()
        .map(|(key, meta)| {
            let channels = state
                .caches
                .instance_channels
                .get(key)
                .map(|v| v.len())
                .unwrap_or(0);
            let up = state
                .caches
                .instance_channels
                .get(key)
                .map(|v| !v.is_empty())
                .unwrap_or(false);
            (
                key.clone(),
                json!({ "maintainer": meta.maintainer, "channels": channels, "up": up }),
            )
        })
        .collect();

    Json(json!({
        "instances": instances,
        "lastUpdate": state.last_updated_ms(),
        "nextUpdate": RELOAD_INTERVAL_MS,
        "uptime": state.started_at_ms,
    }))
    .into_response()
}

#[derive(Deserialize)]
pub struct SearchQuery {
    q: Option<String>,
}

fn bad_request(message: &str) -> Response {
    (
        axum::http::StatusCode::BAD_REQUEST,
        Json(json!({ "error": message })),
    )
        .into_response()
}

pub async fn search(
    State(state): State<Arc<AppState>>,
    Query(params): Query<SearchQuery>,
) -> Response {
    let query = params
        .q
        .unwrap_or_default()
        .trim()
        .trim_start_matches('#')
        .to_ascii_lowercase();

    if query.is_empty() {
        return bad_request("Missing search query");
    }

    if query.len() > search::MAX_QUERY_LEN {
        return bad_request("Search query too long");
    }

    let channels = state
        .caches
        .search_index()
        .search(&query, search::MAX_RESULTS);

    Json(json!({ "query": query, "channels": channels })).into_response()
}
