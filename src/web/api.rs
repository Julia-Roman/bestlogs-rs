use std::collections::HashMap;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderName, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use dashmap::DashMap;
use serde::ser::{SerializeMap, SerializeSeq};
use serde::{Deserialize, Serialize, Serializer};
use serde_json::json;

use crate::logs::mirror::{MirrorOutcome, extract_value, mirror_request};
use crate::logs::{
    instance::get_instance, namehistory::get_name_history, recent_messages::get_recent_messages,
};
use crate::state::AppState;
use crate::stats::spawn_stats;
use crate::util::{self, CHANNEL_LINK_REGEX, USER_LINK_REGEX, is_user_or_channel};

const X_SOURCE: HeaderName = HeaderName::from_static("x-source");

fn request_url(uri: &Uri) -> String {
    uri.path_and_query()
        .map(|p| p.as_str().to_string())
        .unwrap_or_else(|| uri.path().to_string())
}

fn json_status<T: serde::Serialize>(status: u16, body: T) -> Response {
    let code = StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (code, Json(body)).into_response()
}

fn text_status(status: StatusCode, body: impl Into<String>) -> Response {
    let mut response = (status, body.into()).into_response();
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, "text/plain".parse().unwrap());
    response
}

/// Serializes a `DashMap` in place, as a JSON object.
///
/// `/instances` and `/channels` return every channel known to every instance
/// — a few hundred thousand entries. Collecting them into an owned map and
/// handing that to `json!` made two further full copies before the serializer
/// ever saw the data: a deep clone of every `Channel`, then a
/// `serde_json::Value` tree several times larger again, both live at once and
/// once per concurrent request. Borrowing straight from the cache costs
/// nothing but a brief per-shard read lock.
///
/// Key order follows `DashMap`'s iteration rather than the sorted order the
/// `json!` happened to produce; JSON object order is not significant and no
/// client depends on it.
struct MapEntries<'a, V>(&'a DashMap<String, V>);

impl<V: Serialize> Serialize for MapEntries<'_, V> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for entry in self.0.iter() {
            map.serialize_entry(entry.key(), entry.value())?;
        }
        map.end()
    }
}

/// The same, as a JSON array of just the values.
struct MapValues<'a, V>(&'a DashMap<String, V>);

impl<V: Serialize> Serialize for MapValues<'_, V> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(Some(self.0.len()))?;
        for entry in self.0.iter() {
            seq.serialize_element(entry.value())?;
        }
        seq.end()
    }
}

#[derive(Serialize)]
struct InstancesStats {
    count: usize,
    down: usize,
}

#[derive(Serialize)]
struct InstancesBody<'a> {
    instances: MapEntries<'a, Vec<crate::logs::Channel>>,
    #[serde(rename = "instancesStats")]
    instances_stats: InstancesStats,
}

#[derive(Serialize)]
struct ChannelsBody<'a> {
    channels: MapValues<'a, crate::logs::Channel>,
    #[serde(rename = "instancesStats")]
    instances_stats: InstancesStats,
}

fn with_source(mut response: Response, source: &impl serde::Serialize) -> Response {
    if let Ok(value) = serde_json::to_string(source)
        && let Ok(header_value) = value.parse()
    {
        response.headers_mut().insert(X_SOURCE, header_value);
    }
    response
}

pub async fn health(State(state): State<Arc<AppState>>, headers: HeaderMap, uri: Uri) -> Response {
    let start = std::time::Instant::now();
    spawn_stats(
        state.clone(),
        &headers,
        &request_url(&uri),
        "health",
        json!({}),
    );

    let (count, down) = state.caches.stats();
    let channel_count = state.caches.unique_channels.len();
    let instances: HashMap<String, usize> = state
        .caches
        .instance_channels
        .iter()
        .map(|entry| (entry.key().clone(), entry.value().len()))
        .collect();

    let status = if channel_count == 0 { 500 } else { 200 };

    json_status(
        status,
        json!({
            "elapsed": crate::logs::Elapsed::since(start),
            "instancesStats": { "count": count, "down": down },
            "instances": instances,
            "channels": channel_count,
            "instance": state.config.instance,
        }),
    )
}

pub async fn instances(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    spawn_stats(
        state.clone(),
        &headers,
        &request_url(&uri),
        "instances",
        json!({}),
    );

    let (count, down) = state.caches.stats();

    json_status(
        200,
        InstancesBody {
            instances: MapEntries(&state.caches.instance_channels),
            instances_stats: InstancesStats { count, down },
        },
    )
}

pub async fn channels(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    spawn_stats(
        state.clone(),
        &headers,
        &request_url(&uri),
        "channels",
        json!({}),
    );

    let (count, down) = state.caches.stats();

    json_status(
        200,
        ChannelsBody {
            channels: MapValues(&state.caches.unique_channels),
            instances_stats: InstancesStats { count, down },
        },
    )
}

#[derive(Debug, Deserialize)]
pub struct ApiQuery {
    pretty: Option<String>,
    plain: Option<String>,
}

fn pretty_bool(pretty: &Option<String>) -> bool {
    pretty
        .as_deref()
        .map(|s| s.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

async fn api_lookup(
    state: Arc<AppState>,
    headers: HeaderMap,
    uri: Uri,
    channel_raw: String,
    user_raw: Option<String>,
    query: ApiQuery,
) -> Response {
    let channel = util::format_username(&channel_raw);
    let user = user_raw.as_deref().map(util::format_username);

    let mut stat_data = json!({ "channel": channel.clone() });
    if let Some(user) = &user {
        stat_data["user"] = json!(user);
    }
    spawn_stats(
        state.clone(),
        &headers,
        &request_url(&uri),
        "api",
        stat_data,
    );

    let mut error = None;
    if !is_user_or_channel(&channel) {
        error = Some(format!("Invalid channel or channel ID: {channel}"));
    }
    if let Some(user) = &user
        && !is_user_or_channel(user)
    {
        error = Some(format!("Invalid username or user ID: {user}"));
    }

    let is_plain = query
        .plain
        .as_deref()
        .map(|s| s.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    let pretty = pretty_bool(&query.pretty);

    let result = get_instance(&state, &channel, user.as_deref(), false, pretty, error).await;

    if is_plain {
        let code = StatusCode::from_u16(result.status).unwrap_or(StatusCode::BAD_REQUEST);
        let source = if user.is_some() {
            result.user_logs.instances.clone()
        } else {
            result.channel_logs.instances.clone()
        };
        let body = if user.is_some() {
            result.user_logs.full_link.first().cloned()
        } else {
            result.channel_logs.full_link.first().cloned()
        }
        .or_else(|| result.error.clone())
        .unwrap_or_default();

        with_source(text_status(code, body), &source)
    } else {
        json_status(result.status, result)
    }
}

pub async fn api_channel(
    State(state): State<Arc<AppState>>,
    Path(channel): Path<String>,
    Query(query): Query<ApiQuery>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    api_lookup(state, headers, uri, channel, None, query).await
}

pub async fn api_channel_user(
    State(state): State<Arc<AppState>>,
    Path((channel, user)): Path<(String, String)>,
    Query(query): Query<ApiQuery>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    api_lookup(state, headers, uri, channel, Some(user), query).await
}

fn error_redirect(status: u16, message: &str) -> Response {
    let location = format!(
        "/error?code={status}&message={}",
        urlencoding::encode(message)
    );
    (StatusCode::FOUND, [(header::LOCATION, location)]).into_response()
}

#[derive(Debug, Deserialize)]
pub struct RdrQuery {
    pretty: Option<String>,
}

async fn rdr_lookup(
    state: Arc<AppState>,
    headers: HeaderMap,
    channel_raw: String,
    user_raw: Option<String>,
    query: RdrQuery,
) -> Response {
    let channel = util::format_username(&channel_raw);
    let user = user_raw.as_deref().map(util::format_username);

    if !is_user_or_channel(&channel) {
        return error_redirect(400, &format!("Invalid channel or channel ID: {channel}"));
    }
    if let Some(user) = &user
        && !is_user_or_channel(user)
    {
        return error_redirect(400, &format!("Invalid username or user ID: {user}"));
    }

    let pretty = pretty_bool(&query.pretty);
    let result = get_instance(&state, &channel, user.as_deref(), false, pretty, None).await;

    if let Some(error) = result.error {
        return error_redirect(result.status, &error);
    }

    let mut stat_data = json!({ "channel": channel.clone() });
    if let Some(user) = &user {
        stat_data["user"] = json!(user);
    }
    spawn_stats(
        state.clone(),
        &headers,
        &format!("/rdr/{channel_raw}"),
        "rdr",
        stat_data,
    );

    let target = if user.is_some() {
        result.user_logs.full_link.first().cloned()
    } else {
        result.channel_logs.full_link.first().cloned()
    };

    match target {
        Some(target) => (StatusCode::FOUND, [(header::LOCATION, target)]).into_response(),
        None => error_redirect(404, "No channel logs found"),
    }
}

pub async fn rdr_channel(
    State(state): State<Arc<AppState>>,
    Path(channel): Path<String>,
    Query(query): Query<RdrQuery>,
    headers: HeaderMap,
) -> Response {
    rdr_lookup(state, headers, channel, None, query).await
}

pub async fn rdr_channel_user(
    State(state): State<Arc<AppState>>,
    Path((channel, user)): Path<(String, String)>,
    Query(query): Query<RdrQuery>,
    headers: HeaderMap,
) -> Response {
    rdr_lookup(state, headers, channel, Some(user), query).await
}

pub async fn namehistory(
    State(state): State<Arc<AppState>>,
    Path(user): Path<String>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    let mut stat_data = json!({});
    if let Some(login) = user.strip_prefix("login:") {
        stat_data["login"] = json!(login);
    } else {
        stat_data["id"] = json!(user.strip_prefix("id:").unwrap_or(&user));
    }
    spawn_stats(
        state.clone(),
        &headers,
        &request_url(&uri),
        "namehistory",
        stat_data,
    );

    match get_name_history(&state, &user).await {
        Ok(result) => {
            let response = (StatusCode::OK, Json(result.name_history)).into_response();
            with_source(response, &result.source_instances)
        }
        Err(message) => json_status(400, json!({ "error": message })),
    }
}

pub async fn recent_messages(
    State(state): State<Arc<AppState>>,
    Path(channel): Path<String>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    let channel = util::format_username(&channel);
    spawn_stats(
        state.clone(),
        &headers,
        &request_url(&uri),
        "recent-messages",
        json!({ "channel": channel.clone() }),
    );

    let result = get_recent_messages(&state, &channel, &query).await;
    let status = StatusCode::from_u16(result.status).unwrap_or(StatusCode::BAD_REQUEST);
    let source = result.instance.clone();

    let response = (status, Json(result)).into_response();
    with_source(response, &source)
}

pub async fn mirror(State(state): State<Arc<AppState>>, headers: HeaderMap, uri: Uri) -> Response {
    let raw_url = request_url(&uri);
    let channel = extract_value(&raw_url, &CHANNEL_LINK_REGEX);
    let user = extract_value(&raw_url, &USER_LINK_REGEX);

    let mut stat_data = json!({ "channel": channel.clone().unwrap_or_default() });
    if let Some(user) = &user {
        stat_data["user"] = json!(user);
    }
    spawn_stats(state.clone(), &headers, &raw_url, "mirror", stat_data);

    if channel.is_none() {
        return text_status(StatusCode::NOT_FOUND, "Invalid channel or channel ID");
    }

    match mirror_request(&state, &raw_url).await {
        MirrorOutcome::InvalidChannel => {
            text_status(StatusCode::NOT_FOUND, "Invalid channel or channel ID")
        }
        MirrorOutcome::NotFound { status, message } => text_status(
            StatusCode::from_u16(status).unwrap_or(StatusCode::NOT_FOUND),
            message,
        ),
        MirrorOutcome::InvalidEndpoint => text_status(StatusCode::BAD_REQUEST, "Invalid endpoint"),
        MirrorOutcome::UpstreamError(err) => text_status(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Internal error - {err}"),
        ),
        MirrorOutcome::Proxied {
            status,
            content_type,
            content_length,
            body,
            source,
        } => {
            let content_type =
                content_type.unwrap_or_else(|| "application/octet-stream".to_string());
            let mut response = Response::new(body);
            *response.status_mut() = StatusCode::from_u16(status).unwrap_or(StatusCode::OK);
            if let Ok(value) = content_type.parse() {
                response.headers_mut().insert(header::CONTENT_TYPE, value);
            }
            // Forwarded when upstream declared one; the compression layer
            // strips it again if it ends up compressing the stream.
            if let Some(length) = content_length
                && let Ok(value) = length.to_string().parse()
            {
                response.headers_mut().insert(header::CONTENT_LENGTH, value);
            }
            with_source(response, &source)
        }
    }
}
