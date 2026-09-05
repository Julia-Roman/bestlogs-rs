use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::StreamExt;
use futures::stream::FuturesUnordered;

use serde::{Deserialize, Serialize};
use tracing::{error, info};

use crate::logs::health::Permit;
use crate::logs::{AvailableLogDate, Elapsed};
use crate::reload;
use crate::state::AppState;
use crate::twitch::TwitchUser;
use crate::util;

/// How much longer the fan-out keeps collecting once it has an answer worth
/// returning.
///
/// The instances are probed concurrently, so the useful ones cluster: the
/// grace window is measured from the first usable reply rather than from the
/// start of the request, which lets it adapt to the instance mix instead of
/// assuming one. Every instance that is within this of the quickest to answer
/// still gets ranked; the ones that are not keep running in the background
/// and land in the cache, so the next lookup of that channel ranks them too.
const FANOUT_RESULT_GRACE: Duration = Duration::from_millis(300);

/// Ceiling on the above: however late the first usable answer arrives, an
/// answerable lookup returns by this point. Instances still pending are left
/// running, so what this costs is a possibly incomplete ranking for one
/// request, and what it buys is that one overloaded host can no longer set
/// the response time of every request that touches it.
const FANOUT_SOFT_DEADLINE: Duration = Duration::from_millis(1200);

/// The point at which a lookup answers with whatever it has, even nothing.
/// Above the per-probe ceiling (`http_client::LIST_TIMEOUT`) so that in
/// normal operation the probes' own timeouts are what bound the fan-out.
const FANOUT_HARD_DEADLINE: Duration = Duration::from_secs(6);

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

/// A `/list` probe for the channel's day count.
///
/// Reports the outcome to the health tracker, but only a transport failure
/// (timeout, refused connection, TLS error) or a 5xx counts against the
/// instance. A 403/404 is an answer — for an opted-out or unlogged channel a
/// routine one — and must never trip the breaker.
async fn fetch_list(
    state: &AppState,
    host: &str,
    channel_path: &str,
    channel_clean: &str,
    timeout: Duration,
) -> anyhow::Result<Arc<Vec<AvailableLogDate>>> {
    let started = Instant::now();
    let response = match state
        .http
        .get(format!("https://{host}/list"))
        .query(&[(channel_path, channel_clean)])
        .timeout(timeout)
        .send()
        .await
    {
        Ok(response) => response,
        Err(err) => {
            state.health.record_failure(host, &err.to_string());
            return Err(err.into());
        }
    };

    let status = response.status();
    if status.is_server_error() {
        state.health.record_failure(host, status.as_str());
        anyhow::bail!("{host} responded with {status}");
    }

    let body: ListResponse = response.error_for_status()?.json().await?;
    state.health.record_success(host, started.elapsed());
    Ok(Arc::new(body.available_logs))
}

async fn fetch_user_status(
    state: &AppState,
    host: &str,
    channel_path: &str,
    channel_clean: &str,
    user_path: &str,
    user_clean: &str,
    timeout: Duration,
) -> anyhow::Result<u16> {
    let started = Instant::now();
    let response = match state
        .http
        .get(format!("https://{host}/list"))
        .query(&[(channel_path, channel_clean), (user_path, user_clean)])
        .timeout(timeout)
        .send()
        .await
    {
        Ok(response) => response,
        Err(err) => {
            state.health.record_failure(host, &err.to_string());
            return Err(err.into());
        }
    };

    let status = response.status();
    if status.is_server_error() {
        state.health.record_failure(host, status.as_str());
        anyhow::bail!("{host} responded with {status}");
    }

    state.health.record_success(host, started.elapsed());
    Ok(status.as_u16())
}

/// Resolves this instance's day list for the channel, going through the
/// shared cache unless the caller forced a refresh.
///
/// `None` means the instance cannot contribute to this request at all: its
/// breaker is open and nothing is cached for the channel.
#[allow(clippy::too_many_arguments)]
async fn cached_list(
    state: &AppState,
    host: &str,
    channel: &str,
    channel_path: &str,
    channel_clean: &str,
    cache_key: String,
    force: bool,
    permit: Permit,
) -> Option<Arc<Vec<AvailableLogDate>>> {
    let timeout = match permit {
        Permit::Fetch(timeout) => timeout,
        // Tripped: an already-cached list is still perfectly good to rank
        // with, it just doesn't get refreshed from a host that isn't
        // answering.
        Permit::CachedOnly => return state.caches.list_data.get(&cache_key).await,
    };

    let fetch = fetch_list(state, host, channel_path, channel_clean, timeout);

    if force {
        let fetched = match fetch.await {
            Ok(list) => list,
            Err(err) => {
                error!("[{host}] Failed loading {channel} length: {err}");
                Arc::new(Vec::new())
            }
        };
        state
            .caches
            .list_data
            .insert(cache_key, fetched.clone())
            .await;
        Some(fetched)
    } else {
        match state.caches.list_data.try_get_with(cache_key, fetch).await {
            Ok(list) => Some(list),
            Err(err) => {
                error!("[{host}] Failed loading {channel} length: {err}");
                Some(Arc::new(Vec::new()))
            }
        }
    }
}

/// The same, for the user-availability probe. `None` is "not known" — either
/// the breaker is open with nothing cached, or the probe failed — and leaves
/// the instance contributing channel logs only.
#[allow(clippy::too_many_arguments)]
async fn cached_status(
    state: &AppState,
    host: &str,
    channel: &str,
    channel_path: &str,
    channel_clean: &str,
    user: &str,
    user_path: &str,
    user_clean: &str,
    cache_key: String,
    force: bool,
    permit: Permit,
) -> Option<u16> {
    let timeout = match permit {
        Permit::Fetch(timeout) => timeout,
        Permit::CachedOnly => return state.caches.status_codes.get(&cache_key).await,
    };

    let fetch = fetch_user_status(
        state,
        host,
        channel_path,
        channel_clean,
        user_path,
        user_clean,
        timeout,
    );

    if force {
        let resolved = fetch.await.ok()?;
        state.caches.status_codes.insert(cache_key, resolved).await;
        Some(resolved)
    } else {
        match state
            .caches
            .status_codes
            .try_get_with(cache_key, fetch)
            .await
        {
            Ok(status) => Some(status),
            Err(err) => {
                error!("[{host}] Failed checking {channel}/{user} status: {err}");
                None
            }
        }
    }
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
///
/// Takes owned arguments because `get_instance` spawns this per instance:
/// a probe that outlives the request's deadline is left running to populate
/// the cache rather than cancelled.
async fn get_logs(
    state: Arc<AppState>,
    key: String,
    user: Option<String>,
    channel: String,
    force: bool,
    pretty: bool,
    banned: bool,
) -> GetLogsOutcome {
    let channel_path = if util::USER_ID_REGEX.is_match(&channel) {
        "channelid"
    } else {
        "channel"
    };
    let channel_clean = util::strip_id_prefix(&channel);

    // A binary search against the live map entry rather than a scan of it
    // (let alone a cloned-out `Vec`). Every alive instance is probed on
    // every lookup and the lists total ~1.6M channels, so a linear
    // membership test made each request do ~1.6M string comparisons before
    // it could touch the network. The guard is dropped before the first
    // `.await` below, so it can't hold a shard lock against `reload`'s
    // writes. See `logs/channels.rs` for how the entry is ordered.
    let (channel_known, instance_down) = match state.caches.instance_channels.get(&key) {
        Some(entry) => (entry.contains(channel_clean), entry.is_empty()),
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

    // Only resolved once this instance is actually going to be queried —
    // it allocates, and the two early returns above are the common case.
    let host = state.instance_host(&key);
    let permit = state.health.permit(&host);
    let list_cache_key = format!("logs:list:{key}:{}", channel.replacen("id:", "id-", 1));

    let channel_full = if pretty {
        format!("https://tv.supa.sh/logs?c={channel}")
    } else {
        format!("https://{key}/?channel={channel}")
    };

    let Some(user) = user else {
        let list = cached_list(
            &state,
            &host,
            &channel,
            channel_path,
            channel_clean,
            list_cache_key,
            force,
            permit,
        )
        .await;
        return match list {
            Some(list) => GetLogsOutcome::ChannelOnly {
                link: format!("https://{key}"),
                channel_full,
                list,
            },
            None => GetLogsOutcome::Down,
        };
    };

    let instance_cache_key = format!(
        "logs:instance:{key}:{}:{}",
        channel.replacen("id:", "id-", 1),
        user.replacen("id:", "id-", 1)
    );
    let user_path = if util::USER_ID_REGEX.is_match(&user) {
        "userid"
    } else {
        "user"
    };
    let user_clean = util::strip_id_prefix(&user);

    // The day list and the user-availability probe are independent GETs to
    // the same host, so issue them together. Run sequentially, a cold user
    // lookup paid two full round-trips at every instance, and the slowest
    // instance set the latency of the whole fan-out.
    let (list, status_code) = tokio::join!(
        cached_list(
            &state,
            &host,
            &channel,
            channel_path,
            channel_clean,
            list_cache_key,
            force,
            permit,
        ),
        cached_status(
            &state,
            &host,
            &channel,
            channel_path,
            channel_clean,
            &user,
            user_path,
            user_clean,
            instance_cache_key,
            force,
            permit,
        )
    );

    let Some(list) = list else {
        return GetLogsOutcome::Down;
    };

    let full_link = if pretty {
        format!("https://tv.supa.sh/logs?c={channel}&u={user}")
    } else {
        format!("https://{key}/?channel={channel}&username={user}")
    };

    if status_code == Some(403) {
        return GetLogsOutcome::OptedOut {
            link: format!("https://{key}"),
        };
    }

    if status_code.is_some_and(|status| status / 100 == 2) {
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
    let mut abandoned = 0usize;

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
        // Each instance is probed on its own task rather than as one big
        // `join_all`, so a probe that is still running when the deadline
        // below expires is *left running* instead of cancelled: it finishes
        // into the shared cache, and the next request for that channel gets
        // it for free.
        let mut probes: FuturesUnordered<_> = alive
            .iter()
            .map(|key| {
                tokio::spawn(get_logs(
                    state.clone(),
                    key.to_string(),
                    resolved_user.clone(),
                    channel.clone(),
                    force,
                    pretty,
                    banned,
                ))
            })
            .collect();

        // Until the request has an answer worth returning, the wait is bounded
        // by the hard deadline; from that point on it is bounded by the grace
        // window instead. A forced refresh is an explicit "go get me fresh
        // data" and never arms the grace window, so it waits for everything.
        let started_at = tokio::time::Instant::now();
        let soft_deadline = started_at + FANOUT_SOFT_DEADLINE;
        let mut deadline = Box::pin(tokio::time::sleep_until(started_at + FANOUT_HARD_DEADLINE));
        let mut grace_armed = force;

        while !probes.is_empty() {
            tokio::select! {
                biased;

                Some(joined) = probes.next() => {
                    match joined {
                        Ok(GetLogsOutcome::Down) => down_sites += 1,
                        Ok(GetLogsOutcome::Available { link, channel_full, full, list }) => {
                            channel_with_len.push((link.clone(), channel_full, list.clone()));
                            user_with_len.push((link, full, list));
                        }
                        Ok(GetLogsOutcome::ChannelOnly { link, channel_full, list }) => {
                            channel_with_len.push((link, channel_full, list));
                        }
                        Ok(GetLogsOutcome::ChannelNotFound) => {}
                        Ok(GetLogsOutcome::OptedOut { link }) => opt_outs.push(link),
                        Err(err) => {
                            down_sites += 1;
                            error!("[Logs] Instance probe failed: {err}");
                        }
                    }
                }
                _ = &mut deadline => break,
            }

            // Cutting the fan-out short is only worth it once there is an
            // answer to the question that was actually asked: for a user
            // lookup, channel logs alone would still report "no user logs
            // found" while a slow instance was about to say otherwise.
            let answered = !channel_with_len.is_empty()
                && (resolved_user.is_none() || !user_with_len.is_empty());
            if answered && !grace_armed {
                grace_armed = true;
                deadline
                    .as_mut()
                    .reset((tokio::time::Instant::now() + FANOUT_RESULT_GRACE).min(soft_deadline));
            }
        }

        // Stragglers count as down for this request only — they are still
        // running, and whatever they return lands in the cache.
        abandoned = probes.len();
        down_sites += abandoned;

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
        "[Logs] Channel: {channel}{} | {:.2}ms{}",
        resolved_user
            .as_ref()
            .map(|u| format!(" - User: {u}"))
            .unwrap_or_default(),
        start.elapsed().as_secs_f64() * 1000.0,
        if abandoned > 0 {
            format!(" | answered without {abandoned} slow instance(s)")
        } else {
            String::new()
        }
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
