use std::sync::Arc;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use tracing::{error, info};

use crate::logs::{AvailableLogDate, Elapsed};
use crate::state::AppState;
use crate::twitch::TwitchUser;
use crate::util;
use crate::{http_client, reload};

#[derive(Deserialize)]
struct ListResponse {
    #[serde(default, rename = "availableLogs")]
    available_logs: Vec<AvailableLogDate>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestPersonInfo {
    pub login: String,
    pub id: String,
    pub banned: bool,
}

impl From<&TwitchUser> for RequestPersonInfo {
    fn from(u: &TwitchUser) -> Self {
        RequestPersonInfo {
            login: u.login.clone(),
            id: u.id.clone(),
            banned: u.banned,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct RequestInfo {
    pub channel: Option<RequestPersonInfo>,
    pub user: Option<RequestPersonInfo>,
    pub forced: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Available {
    pub user: bool,
    pub channel: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoggedData {
    pub list: Arc<Vec<AvailableLogDate>>,
    pub days: usize,
    pub since: Option<AvailableLogDate>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LinksBlock {
    pub count: usize,
    pub instances: Vec<String>,
    pub full_link: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct OptedOut {
    pub count: usize,
    pub instances: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LastUpdated {
    pub unix: i64,
    pub utc: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstancesInfo {
    pub count: usize,
    pub down: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstanceResult {
    pub error: Option<String>,
    pub status: u16,
    pub instances_info: InstancesInfo,
    pub request: RequestInfo,
    pub available: Available,
    pub logged_data: LoggedData,
    pub user_logs: LinksBlock,
    pub channel_logs: LinksBlock,
    pub opted_out: OptedOut,
    pub last_updated: LastUpdated,
    pub elapsed: Elapsed,
}

/// One instance's ranking entry: its display link, the user-facing full
/// link, and the shared date list its rank is derived from.
type RankedInstance = (String, String, Arc<Vec<AvailableLogDate>>);

/// Outcome of probing a single instance for a channel (and optionally a
/// user)'s logs. An enum instead of a status code + loosely-related
/// `Option` fields, so a caller can never observe an invariant violation
/// (e.g. "status says available but the link is missing") — the compiler
/// guarantees every variant carries exactly the data it needs.
enum GetLogsOutcome {
    /// The instance has no working channel list at all right now.
    Down,
    /// The instance is up but doesn't have this channel.
    ChannelNotFound,
    /// The instance is up and has the channel, but the user (or channel)
    /// opted out of logging.
    OptedOut { link: String },
    /// The channel is logged; no user was requested (or its status wasn't
    /// checked).
    ChannelOnly {
        link: String,
        channel_full: String,
        list: Arc<Vec<AvailableLogDate>>,
    },
    /// The channel (and, if requested, the user) are logged here.
    Available {
        link: String,
        channel_full: String,
        full: String,
        list: Arc<Vec<AvailableLogDate>>,
    },
}

async fn fetch_list(
    state: &AppState,
    host: &str,
    channel_path: &str,
    channel_clean: &str,
) -> anyhow::Result<Arc<Vec<AvailableLogDate>>> {
    let response = state
        .http
        .get(format!("https://{host}/list"))
        .query(&[(channel_path, channel_clean)])
        .timeout(http_client::LIST_TIMEOUT)
        .send()
        .await?
        .error_for_status()?;
    let body: ListResponse = response.json().await?;
    Ok(Arc::new(body.available_logs))
}

async fn fetch_user_status(
    state: &AppState,
    host: &str,
    channel_path: &str,
    channel_clean: &str,
    user_path: &str,
    user_clean: &str,
) -> anyhow::Result<u16> {
    let response = state
        .http
        .get(format!("https://{host}/list"))
        .query(&[(channel_path, channel_clean), (user_path, user_clean)])
        .timeout(http_client::LIST_TIMEOUT)
        .send()
        .await?;
    Ok(response.status().as_u16())
}

/// Port of `Utils.getLogs`: probes a single instance for a channel (and
/// optionally a user)'s logs and classifies availability.
///
/// The `/list` probes are cached with `try_get_with` (see `state.rs` for the
/// TTL): concurrent requests for the same channel on the same instance
/// coalesce into a single upstream call instead of each firing their own,
/// and a failed probe is never cached — only a genuine answer (including a
/// genuinely empty list) is, so a timeout or 5xx self-heals on the very
/// next request rather than being stuck for the rest of the cache's TTL.
async fn get_logs(
    state: &AppState,
    key: &str,
    user: Option<&str>,
    channel: &str,
    force: bool,
    pretty: bool,
    banned: bool,
) -> GetLogsOutcome {
    let channel_path = if util::USER_ID_REGEX.is_match(channel) {
        "channelid"
    } else {
        "channel"
    };
    let host = state.instance_host(key);
    let channel_clean = util::strip_id_prefix(channel);

    // Scanned against the live map entry rather than a cloned-out `Vec`.
    // Materialising the list here cost ~2*N `String` allocations per instance
    // per request, with every alive instance's copy live at once (they're
    // probed concurrently) — tens of megabytes of churn to answer a single
    // bool, and the dominant source of the resident-memory blowup under load.
    // The guard is dropped before the first `.await` below, so it can't hold
    // a shard lock against `reload`'s writes.
    let (channel_known, instance_down) = match state.caches.instance_channels.get(key) {
        Some(entry) => (
            entry
                .iter()
                .any(|c| c.name == channel_clean || c.user_id == channel_clean),
            entry.is_empty(),
        ),
        // Not loaded yet: matches the previous `unwrap_or_default()` empty
        // list — nothing known, and reported down for a banned-channel lookup.
        None => (false, true),
    };

    if !banned && !channel_known {
        return GetLogsOutcome::ChannelNotFound;
    }
    if instance_down {
        return GetLogsOutcome::Down;
    }

    let list_cache_key = format!("logs:list:{key}:{}", channel.replacen("id:", "id-", 1));

    let list = if force {
        let fetched = fetch_list(state, &host, channel_path, channel_clean)
            .await
            .unwrap_or_else(|err| {
                error!("[{host}] Failed loading {channel} length: {err}");
                Arc::new(Vec::new())
            });
        state
            .caches
            .list_data
            .insert(list_cache_key, fetched.clone())
            .await;
        fetched
    } else {
        match state
            .caches
            .list_data
            .try_get_with(
                list_cache_key,
                fetch_list(state, &host, channel_path, channel_clean),
            )
            .await
        {
            Ok(list) => list,
            Err(err) => {
                error!("[{host}] Failed loading {channel} length: {err}");
                Arc::new(Vec::new())
            }
        }
    };

    let channel_full = if pretty {
        format!("https://tv.supa.sh/logs?c={channel}")
    } else {
        format!("https://{key}/?channel={channel}")
    };

    let Some(user) = user else {
        return GetLogsOutcome::ChannelOnly {
            link: format!("https://{key}"),
            channel_full,
            list,
        };
    };

    let instance_cache_key = format!(
        "logs:instance:{key}:{}:{}",
        channel.replacen("id:", "id-", 1),
        user.replacen("id:", "id-", 1)
    );
    let user_path = if util::USER_ID_REGEX.is_match(user) {
        "userid"
    } else {
        "user"
    };
    let user_clean = util::strip_id_prefix(user);

    let status_code = if force {
        let resolved = fetch_user_status(
            state,
            &host,
            channel_path,
            channel_clean,
            user_path,
            user_clean,
        )
        .await
        .unwrap_or(500);
        state
            .caches
            .status_codes
            .insert(instance_cache_key, resolved)
            .await;
        resolved
    } else {
        match state
            .caches
            .status_codes
            .try_get_with(
                instance_cache_key,
                fetch_user_status(
                    state,
                    &host,
                    channel_path,
                    channel_clean,
                    user_path,
                    user_clean,
                ),
            )
            .await
        {
            Ok(status) => status,
            Err(err) => {
                error!("[{host}] Failed checking {channel}/{user} status: {err}");
                500
            }
        }
    };

    let full_link = if pretty {
        format!("https://tv.supa.sh/logs?c={channel}&u={user}")
    } else {
        format!("https://{key}/?channel={channel}&username={user}")
    };

    if status_code == 403 {
        return GetLogsOutcome::OptedOut {
            link: format!("https://{key}"),
        };
    }

    if status_code / 100 == 2 {
        GetLogsOutcome::Available {
            link: format!("https://{key}"),
            channel_full,
            full: full_link,
            list,
        }
    } else {
        GetLogsOutcome::ChannelOnly {
            link: format!("https://{key}"),
            channel_full,
            list,
        }
    }
}

/// Port of `Utils.getInstance`: resolves the channel (and optional user) via
/// api.ivr.fi, ranks every alive instance by log-day count, and assembles
/// the same aggregate response shape as the original.
pub async fn get_instance(
    state: &Arc<AppState>,
    channel: &str,
    user: Option<&str>,
    force: bool,
    pretty: bool,
    pre_error: Option<String>,
) -> InstanceResult {
    let start = Instant::now();
    let mut error = pre_error;
    let mut status: u16 = 200;
    let mut down_sites = 0usize;

    let mut request = RequestInfo {
        channel: None,
        user: None,
        forced: force,
    };

    if force {
        reload::load_instance_channels(state, false).await;
    }

    // Channel and (if any) user are independent lookups against ivr.fi, so
    // resolve them concurrently instead of one after the other.
    let (channel_info, user_info) = tokio::join!(crate::twitch::get_info(state, channel), async {
        match user {
            Some(u) => Some(crate::twitch::get_info(state, u).await),
            None => None,
        }
    });
    let channel_info = channel_info.ok();

    let mut channel = channel.to_string();
    let mut banned = false;
    if let Some(info) = &channel_info {
        request.channel = Some(info.into());
        banned = info.banned;
        if info.banned {
            channel = format!("id:{}", info.id);
        }
    } else {
        error = Some(format!("The channel does not exist: {channel}"));
    }

    let mut resolved_user: Option<String> = user.map(str::to_string);
    if let Some(user) = user {
        match user_info {
            Some(Ok(info)) => {
                request.user = Some((&info).into());
                if info.banned {
                    resolved_user = Some(format!("id:{}", info.id));
                }
            }
            _ => {
                error = Some(format!("The user does not exist: {user}"));
            }
        }
    }

    let mut opt_outs: Vec<String> = Vec::new();
    let mut user_links: Vec<String> = Vec::new();
    let mut channel_links: Vec<String> = Vec::new();
    let mut user_instances: Vec<String> = Vec::new();
    let mut channel_instances: Vec<String> = Vec::new();
    let mut user_with_len: Vec<RankedInstance> = Vec::new();
    let mut channel_with_len: Vec<RankedInstance> = Vec::new();

    if error.is_none() {
        let alive = state.alive_instances();
        let results = futures::future::join_all(alive.iter().map(|key| {
            get_logs(
                state,
                key,
                resolved_user.as_deref(),
                &channel,
                force,
                pretty,
                banned,
            )
        }))
        .await;

        for outcome in results {
            match outcome {
                GetLogsOutcome::Down => down_sites += 1,
                GetLogsOutcome::Available {
                    link,
                    channel_full,
                    full,
                    list,
                } => {
                    channel_with_len.push((link.clone(), channel_full, list.clone()));
                    user_with_len.push((link, full, list));
                }
                GetLogsOutcome::ChannelOnly {
                    link,
                    channel_full,
                    list,
                } => {
                    channel_with_len.push((link, channel_full, list));
                }
                GetLogsOutcome::ChannelNotFound => {}
                GetLogsOutcome::OptedOut { link } => opt_outs.push(link),
            }
        }

        channel_with_len.sort_by_key(|a| std::cmp::Reverse(a.2.len()));
        user_with_len.sort_by_key(|a| std::cmp::Reverse(a.2.len()));

        for (link, full, _) in &channel_with_len {
            channel_instances.push(link.clone());
            channel_links.push(full.clone());
        }
        for (link, full, _) in &user_with_len {
            user_instances.push(link.clone());
            user_links.push(full.clone());
        }

        if !opt_outs.is_empty() && channel_instances.is_empty() {
            error = Some("User or channel has opted out".to_string());
            status = 403;
        } else if channel_instances.is_empty() {
            error = Some("No channel logs found".to_string());
            status = 404;
        } else if user_instances.is_empty() && resolved_user.is_some() {
            error = Some("No user logs found".to_string());
            status = 404;
        }
    } else {
        status = 404;
    }

    let channel_list = channel_with_len
        .first()
        .map(|(_, _, list)| list.clone())
        .unwrap_or_default();

    if let Some(info) = &channel_info
        && info.banned
        && !channel_instances.is_empty()
    {
        state.caches.unique_channels.insert(
            info.id.clone(),
            crate::logs::Channel {
                name: info.login.clone(),
                user_id: info.id.clone(),
            },
        );
    }

    info!(
        "[Logs] Channel: {channel}{} | {:.2}ms",
        resolved_user
            .as_ref()
            .map(|u| format!(" - User: {u}"))
            .unwrap_or_default(),
        start.elapsed().as_secs_f64() * 1000.0
    );

    let last_updated_ms = state.last_updated_ms();
    let since = channel_list.last().cloned();
    let days = channel_list.len();

    InstanceResult {
        error,
        status,
        instances_info: InstancesInfo {
            count: state.config.justlogs_instances.len(),
            down: down_sites,
        },
        request,
        available: Available {
            user: !user_instances.is_empty(),
            channel: !channel_instances.is_empty(),
        },
        logged_data: LoggedData {
            list: channel_list,
            days,
            since,
        },
        user_logs: LinksBlock {
            count: user_instances.len(),
            instances: user_instances,
            full_link: user_links,
        },
        channel_logs: LinksBlock {
            count: channel_instances.len(),
            instances: channel_instances,
            full_link: channel_links,
        },
        opted_out: OptedOut {
            count: opt_outs.len(),
            instances: opt_outs,
        },
        last_updated: LastUpdated {
            unix: last_updated_ms / 1000,
            utc: util::to_utc_string(last_updated_ms),
        },
        elapsed: Elapsed::since(start),
    }
}
