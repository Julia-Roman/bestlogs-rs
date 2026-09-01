use std::collections::HashMap;
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tracing::{error, info};

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

async fn fetch_messages(
    state: &AppState,
    instance: &str,
    channel: &str,
    query: &str,
) -> (u16, RmResponse) {
    let mut url = format!("https://{instance}/api/v2/recent-messages/{channel}");
    if !query.is_empty() {
        url.push('?');
        url.push_str(query);
    }

    let response = state
        .http
        .get(&url)
        .timeout(RECENT_MESSAGES_TIMEOUT)
        .send()
        .await;
    match response {
        Ok(res) => {
            let status = res.status().as_u16();
            let body = res.json::<RmResponse>().await.unwrap_or_default();
            (status, body)
        }
        Err(_) => (500, RmResponse::default()),
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
    channel: &str,
    date: &AvailableLogDate,
    limit: usize,
    first_ts: Option<i64>,
) -> anyhow::Result<Vec<String>> {
    let day = date.day.as_deref().unwrap_or("1");
    let url = format!(
        "{instance_link}/channel/{channel}/{}/{}/{day}?limit={limit}&raw&reverse",
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
    lines.reverse();
    if !lines.is_empty() {
        lines.remove(0);
    }

    Ok(tag_historical(lines, first_ts))
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
        let (status_code, body) = fetch_messages(state, entry, channel, &raw_query).await;
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
        } else {
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
    }

    let mut instance = vec![rm_instance];
    let first_ts: Option<i64> = messages
        .first()
        .and_then(|m| TMI_SENT_REGEX.captures(m))
        .and_then(|c| c[1].parse().ok());

    if !rm_only {
        let logs = get_instance(state, channel, None, false, false, None).await;

        let mut instances = logs.channel_logs.instances.clone();
        if let Some(first_key) = state.config.justlogs_instances.keys().next() {
            let main_instance = format!("https://{first_key}");
            if let Some(index) = instances.iter().position(|i| i == &main_instance)
                && index > 0
            {
                let picked = instances.remove(index);
                instances.insert(0, picked);
            }
        }

        let mut success = false;
        let mut retries = 0usize;
        let mut instance_index = 0usize;
        #[allow(unused_assignments)]
        let mut instance_link = "Logs".to_string();

        while retries < MAX_RETRIES && instance_index < instances.len() && !success {
            instance_link = instances[instance_index].clone();

            let attempt: anyhow::Result<()> = async {
                if !logs.available.channel {
                    return Ok(());
                }

                let list = &logs.logged_data.list;
                let mut days: Vec<Vec<String>> = Vec::new();
                let mut total = messages.len();
                let mut days_fetched = 0usize;

                while total < limit && days_fetched < MAX_BACKFILL_DAYS && days_fetched < list.len()
                {
                    match fetch_rustlog_day(
                        state,
                        &instance_link,
                        channel,
                        &list[days_fetched],
                        limit - total,
                        first_ts,
                    )
                    .await
                    {
                        Ok(day_logs) => {
                            total += day_logs.len();
                            days.push(day_logs);
                        }
                        Err(err) => {
                            if days_fetched == 0 {
                                return Err(err);
                            }
                        }
                    }
                    days_fetched += 1;
                }

                info!(
                    "[{}] Channel: {channel} | 200 - {total} messages",
                    instance_link.replace("https://", "")
                );

                // Days are fetched newest first and the response wants oldest
                // first, with the live recent-messages tail last. Assembled
                // once into a right-sized buffer: prepending each day with
                // `chain(..).collect()` reallocated and re-copied the whole
                // accumulated list on every one of the (up to 7) iterations.
                let mut merged = Vec::with_capacity(total);
                for day in days.into_iter().rev() {
                    merged.extend(day);
                }
                // Takes ownership of the recent-messages tail instead of
                // cloning it. The old guard here compared the merged length
                // against `messages.len()`, which it is by construction never
                // smaller than.
                merged.append(&mut messages);

                instance.push(instance_link.clone());
                messages = merged;
                error_code = None;
                error = None;
                status = 200;
                success = true;

                Ok(())
            }
            .await;

            if let Err(err) = attempt {
                error!(
                    "[{}] Channel: {channel} | Failed loading messages: {err}",
                    instance_link.replace("https://", "")
                );
                retries += 1;
            }
            instance_index += 1;
        }

        if !success {
            error!(
                "[RecentMessages] Channel: {channel} | Failed to fetch logs after {} retries",
                retries + 1
            );
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
