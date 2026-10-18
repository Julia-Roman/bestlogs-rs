use std::collections::HashMap;
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tracing::{error, info};

use crate::logs::health::Permit;
use crate::logs::{AvailableLogDate, Elapsed, instance::get_instance};
use crate::state::AppState;
use crate::util::TMI_SENT_REGEX;

static NEWLINE_REGEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\r?\n").unwrap());

const RECENT_MESSAGES_TIMEOUT: Duration = Duration::from_secs(5);
const RUSTLOG_DAY_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_BACKFILL_DAYS: usize = 7;
const MAX_RETRIES: usize = 3;

/// Hard ceiling on the client-supplied `limit`. It is the only knob a caller
/// has over how much this endpoint allocates: it bounds both how many lines
/// the rustlog backfill asks each day for and how large the assembled
/// response grows, so an unclamped `?limit=999999999` was a single-request
/// out-of-memory. Real clients ask for far less than this — Chatterino
/// requests 800, and recent-messages2 itself caps at 1000 — so the ceiling
/// is generous enough to be invisible in practice.
const MAX_LIMIT: usize = 100_000;

/// Response body for the recent-messages endpoints.
///
/// Serialized straight from this struct rather than built as a
/// `serde_json::Value`: routing every message through a `Value` tree meant a
/// second full copy of the entire message list (each `String` re-boxed into a
/// `Value`, which is several times the size of the line itself) alive at the
/// same time as the first, purely to hand it to the serializer.
///
/// Fields are ordered alphabetically to reproduce the old `json!` output
/// byte for byte — `serde_json` is built without `preserve_order` here, so
/// its maps serialize in `BTreeMap` (sorted-key) order.
#[derive(Debug, Serialize)]
pub struct RecentMessagesResponse {
    pub count: usize,
    pub elapsed: Elapsed,
    pub error: Option<String>,
    pub error_code: Option<String>,
    pub instance: Vec<String>,
    pub messages: Vec<String>,
    pub request: Value,
    pub status: u16,
    pub status_message: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct RmResponse {
    #[serde(default)]
    messages: Vec<String>,
    error: Option<String>,
    error_code: Option<String>,
    status_message: Option<String>,
}

/// `None` means the instance is currently tripped (see `logs/health.rs`) and
/// was skipped without a request. Chatterino opens one of these per joined
/// channel the moment it connects, so an unresponsive first-choice instance
/// used to cost every one of those requests a full timeout before the
/// fallback was even tried.
async fn fetch_messages(
    state: &AppState,
    instance: &str,
    channel: &str,
    query: &str,
) -> Option<(u16, RmResponse)> {
    let timeout = match state.health.permit(instance) {
        Permit::Fetch(timeout) => timeout.min(RECENT_MESSAGES_TIMEOUT),
        Permit::CachedOnly => return None,
    };

    let mut url = format!("https://{instance}/api/v2/recent-messages/{channel}");
    if !query.is_empty() {
        url.push('?');
        url.push_str(query);
    }

    let started = Instant::now();
    let response = state.http.get(&url).timeout(timeout).send().await;
    match response {
        Ok(res) => {
            let status = res.status();
            if status.is_server_error() {
                state.health.record_failure(instance, status.as_str());
            } else {
                state.health.record_success(instance, started.elapsed());
            }
            let body = res.json::<RmResponse>().await.unwrap_or_default();
            Some((status.as_u16(), body))
        }
        Err(err) => {
            state.health.record_failure(instance, &err.to_string());
            Some((500, RmResponse::default()))
        }
    }
}

/// Tags backfilled lines the way Chatterino expects historical replay
/// messages to look, and drops anything already covered by recent-messages
/// (`first_ts`). Port of the tail of `fetchRustlogs`.
fn tag_historical(lines: Vec<String>, first_ts: Option<i64>) -> Vec<String> {
    lines
        .into_iter()
        .filter_map(|message| {
            let mut message = message;
            if let Some(caps) = TMI_SENT_REGEX.captures(&message) {
                let timestamp: i64 = caps[1].parse().unwrap_or(0);
                if let Some(first_ts) = first_ts
                    && timestamp >= first_ts
                {
                    return None;
                }
                let old = format!("tmi-sent-ts={timestamp};");
                let new = format!("tmi-sent-ts={timestamp};rm-received-ts={timestamp};");
                message = message.replacen(&old, &new, 1);
            }
            let mut chars = message.chars();
            chars.next();
            Some(format!("@historical=1;{}", chars.as_str()))
        })
        .collect()
}

/// Port of `fetchRustlogs`: pulls one day's raw IRC log lines from a rustlog
/// instance for backfill.
async fn fetch_rustlog_day(
    state: &AppState,
    instance_link: &str,
    channel_id: &str,
    date: &AvailableLogDate,
    limit: usize,
    first_ts: Option<i64>,
) -> anyhow::Result<Vec<String>> {
    let day = date.day.as_deref().unwrap_or("1");
    let url = format!(
        "{instance_link}/channelid/{channel_id}/{}/{}/{day}?limit={limit}&raw&reverse",
        date.year, date.month
    );

    let body = state
        .http
        .get(&url)
        .timeout(RUSTLOG_DAY_TIMEOUT)
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;

    let mut lines: Vec<String> = NEWLINE_REGEX.split(&body).map(str::to_string).collect();
    lines.pop();
    lines.reverse();

    Ok(tag_historical(lines, first_ts))
}

/// Fetches up to `MAX_BACKFILL_DAYS` of `days` (newest first) from one
/// instance, stopping once `limit` is reached counting the `recent` messages
/// already in hand, and returns them oldest first. Only a failure on the
/// newest day fails the instance; a later day that fails is skipped.
async fn fetch_backfill(
    state: &AppState,
    instance_link: &str,
    channel_id: &str,
    days: &[AvailableLogDate],
    limit: usize,
    recent: usize,
    first_ts: Option<i64>,
) -> anyhow::Result<Vec<String>> {
    let mut fetched: Vec<Vec<String>> = Vec::new();
    let mut total = recent;

    for (index, date) in days.iter().take(MAX_BACKFILL_DAYS).enumerate() {
        if total >= limit {
            break;
        }
        match fetch_rustlog_day(state, instance_link, channel_id, date, limit - total, first_ts)
            .await
        {
            Ok(lines) => {
                total += lines.len();
                fetched.push(lines);
            }
            Err(err) if index == 0 => return Err(err),
            Err(_) => {}
        }
    }

    // Assembled once into a buffer sized for the recent-messages tail too:
    // prepending each day with `chain(..).collect()` reallocated and
    // re-copied the whole accumulated list on every one of the (up to 7)
    // iterations.
    let mut merged = Vec::with_capacity(total);
    for day in fetched.into_iter().rev() {
        merged.extend(day);
    }
    Ok(merged)
}

/// Older history for `channel` from the best-ranked log instance (the first
/// configured one whenever it has the channel), trying the next-ranked one
/// when an instance fails, up to `MAX_RETRIES` failures. Returns the link of
/// the instance it came from along with the lines, oldest first.
async fn backfill(
    state: &Arc<AppState>,
    channel: &str,
    recent: usize,
    limit: usize,
    first_ts: Option<i64>,
) -> Option<(String, Vec<String>)> {
    let logs = get_instance(state, channel, None, false, false, None).await;
    let channel_id = logs.request.channel.as_ref().map_or("", |c| c.id.as_str());

    let mut instances = logs.channel_logs.instances;
    if let Some(first_key) = state.config.justlogs_instances.keys().next() {
        let main_instance = format!("https://{first_key}");
        if let Some(index) = instances.iter().position(|i| *i == main_instance) {
            instances[..=index].rotate_right(1);
        }
    }

    let mut failures = 0usize;
    for link in instances {
        if failures == MAX_RETRIES {
            break;
        }
        let host = link.replace("https://", "");
        let days = &logs.logged_data.list;
        match fetch_backfill(state, &link, channel_id, days, limit, recent, first_ts).await {
            Ok(lines) => {
                info!(
                    "[{host}] Channel: {channel} | 200 - {} messages",
                    recent + lines.len()
                );
                return Some((link, lines));
            }
            Err(err) => {
                error!("[{host}] Channel: {channel} | Failed loading messages: {err}");
                failures += 1;
            }
        }
    }

    error!(
        "[RecentMessages] Channel: {channel} | Failed to fetch logs after {} retries",
        failures + 1
    );
    None
}

/// Port of `Utils.getRecentMessages`: queries recent-messages instances for
/// live history, then (unless `rm_only=true`) backfills older messages from
/// the best-ranked rustlog instance, day by day, up to `limit`/7 days.
pub async fn get_recent_messages(
    state: &Arc<AppState>,
    channel: &str,
    query: &HashMap<String, String>,
) -> RecentMessagesResponse {
    let start = Instant::now();

    let limit: usize = query
        .get("limit")
        .and_then(|v| v.parse().ok())
        .filter(|&n: &usize| n > 0)
        .unwrap_or(1000)
        .min(MAX_LIMIT);
    let rm_only = query.get("rm_only").map(String::as_str) == Some("true");

    let raw_query = serde_urlencoded::to_string(query).unwrap_or_default();

    let mut messages: Vec<String> = Vec::new();
    let mut status_message: Option<String> = None;
    let mut error_code: Option<String> = None;
    let mut error: Option<String> = None;
    let mut status: u16 = 500;
    let mut rm_instance = String::new();

    for entry in state.config.recentmessages_instances.keys() {
        let Some((status_code, body)) = fetch_messages(state, entry, channel, &raw_query).await
        else {
            error!("[{entry}] Channel: {channel} | Skipped, instance is unresponsive");
            continue;
        };
        status_message = body.status_message.clone();
        rm_instance = format!("https://{entry}");
        status = status_code;

        if status_code == 200 && !body.messages.is_empty() {
            let filtered: Vec<String> = body
                .messages
                .into_iter()
                .filter(|m| !m.contains(":tmi.twitch.tv ROOMSTATE #"))
                .collect();
            error_code = body.error_code;
            error = body.error;
            info!(
                "[{entry}] Channel: {channel} | {status_code} - {} messages",
                filtered.len()
            );
            messages = filtered;
            break;
        }

        error_code = Some(
            body.error_code
                .unwrap_or_else(|| "internal_server_error".to_string()),
        );
        error = Some(
            body.error
                .unwrap_or_else(|| "Internal Server Error".to_string()),
        );
        error!(
            "[{entry}] Channel: {channel} | {status_code} - {}",
            error.as_deref().unwrap_or("")
        );
    }

    let mut instance = vec![rm_instance];

    if !rm_only {
        let first_ts: Option<i64> = messages
            .first()
            .and_then(|m| TMI_SENT_REGEX.captures(m))
            .and_then(|c| c[1].parse().ok());

        if let Some((link, mut merged)) =
            backfill(state, channel, messages.len(), limit, first_ts).await
        {
            // Takes ownership of the recent-messages tail instead of
            // cloning it; `merged` was sized for it.
            merged.append(&mut messages);
            messages = merged;
            instance.push(link);
            error_code = None;
            error = None;
            status = 200;
        }
    }

    let mut request = json!({ "channel": channel, "limit": limit });
    if let Value::Object(map) = &mut request {
        for (key, value) in query {
            if key == "channel" || key == "limit" {
                continue;
            }
            let coerced = match value.as_str() {
                "true" => Value::Bool(true),
                "false" => Value::Bool(false),
                other => Value::String(other.to_string()),
            };
            map.insert(key.clone(), coerced);
        }
    }

    info!(
        "[RecentMessages] Channel: {channel} | {status} - [{}/{limit}] | {instance:?} | {:.2}ms",
        messages.len(),
        start.elapsed().as_secs_f64() * 1000.0
    );

    RecentMessagesResponse {
        count: messages.len(),
        elapsed: Elapsed::since(start),
        error,
        error_code,
        instance,
        messages,
        request,
        status,
        status_message,
    }
}
