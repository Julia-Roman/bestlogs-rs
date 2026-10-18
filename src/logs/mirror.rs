use std::collections::BTreeMap;
use std::sync::{Arc, LazyLock};

use axum::body::Body;
use regex::Regex;
use serde::Serialize;

use crate::http_client;
use crate::logs::AvailableLogDate;
use crate::state::AppState;
use crate::util::{self, CHANNEL_LINK_REGEX, USER_LINK_REGEX};

static ID_STYLE_REGEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"id[/=][0-9]+").unwrap());

/// A log request for a specific day (`/channel/{c}/{y}/{m}/{d}`) or month
/// (`/channel/{c}/user/{u}/{y}/{m}`), in either the name or id form.
static DATED_PATH_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^/channel(?:id)?/[^/]+(?:/user(?:id)?/[^/]+)?/([0-9]{4})/([0-9]{1,2})(?:/([0-9]{1,2}))?/?$",
    )
    .unwrap()
});

/// Year, month, day — day 0 for a month entry. Instances don't agree on
/// zero-padding, so dates are compared numerically rather than as strings.
type DateKey = (u32, u32, u32);

fn date_key(date: &AvailableLogDate) -> Option<DateKey> {
    let day = match &date.day {
        Some(day) => day.parse().ok()?,
        None => 0,
    };
    Some((date.year.parse().ok()?, date.month.parse().ok()?, day))
}

#[derive(Serialize)]
struct ListBody {
    #[serde(rename = "availableLogs")]
    available_logs: Vec<AvailableLogDate>,
}

/// Every date any of the instances has, newest first like a single
/// instance's own `/list`.
fn merge_lists(lists: &[Arc<Vec<AvailableLogDate>>]) -> Vec<AvailableLogDate> {
    let mut merged: BTreeMap<DateKey, &AvailableLogDate> = BTreeMap::new();
    for date in lists.iter().flat_map(|list| list.iter()) {
        if let Some(key) = date_key(date) {
            merged.entry(key).or_insert(date);
        }
    }
    merged.into_values().rev().cloned().collect()
}

/// The first instance, in ranked order, whose list has the date the path
/// asks for. A month entry (user lists only go down to the month) covers
/// every day in it. `None` when the path isn't dated or nobody has that date.
fn pick_for_date(
    path: &str,
    instances: &[String],
    lists: &[Arc<Vec<AvailableLogDate>>],
) -> Option<String> {
    let caps = DATED_PATH_REGEX.captures(path)?;
    let year: u32 = caps[1].parse().ok()?;
    let month: u32 = caps[2].parse().ok()?;
    let day: Option<u32> = caps.get(3).and_then(|d| d.as_str().parse().ok());

    let has_date = |list: &Arc<Vec<AvailableLogDate>>| {
        list.iter()
            .filter_map(date_key)
            .any(|(y, m, d)| y == year && m == month && (d == 0 || day.is_none_or(|day| d == day)))
    };
    instances
        .iter()
        .zip(lists)
        .find(|&(_, list)| has_date(list))
        .map(|(instance, _)| instance.clone())
}

/// Port of `extractValue`: pulls a channel/user token out of the raw mirror
/// URL, prefixing it with `id:` whenever the URL uses id-style references
/// anywhere (matching the original's URL-wide, not match-scoped, check).
pub fn extract_value(raw_url: &str, regex: &Regex) -> Option<String> {
    let formatted = util::format_username(raw_url);
    let captures = regex.captures(&formatted)?;
    let value = captures.get(2)?.as_str().to_string();
    if ID_STYLE_REGEX.is_match(&formatted) {
        Some(format!("id:{value}"))
    } else {
        Some(value)
    }
}

pub enum MirrorOutcome {
    /// No channel could be extracted from the URL at all.
    InvalidChannel,
    /// `get_instance` returned an error (channel/user not found, opted out, ...).
    NotFound {
        status: u16,
        message: String,
    },
    /// The upstream endpoint responded with an HTML page instead of API data.
    InvalidEndpoint,
    /// Successful passthrough. The body is the upstream response streamed
    /// through unbuffered — a single `/channel/{name}/{year}/{month}` day of
    /// raw logs can be hundreds of megabytes, and buffering it (plus the
    /// compression layer's copy of it) meant peak memory scaled with the
    /// number of concurrent mirror requests times the largest log a client
    /// could ask for. Nothing here needs the whole body in memory.
    Proxied {
        status: u16,
        content_type: Option<String>,
        content_length: Option<u64>,
        body: Body,
        source: Vec<String>,
    },
    UpstreamError(String),
}

/// Port of `logsApi`: resolves the best instance for the channel/user
/// embedded in the URL, rewrites the path to use resolved Twitch ids, and
/// proxies the request through to that instance. `/list` is the exception:
/// it is answered locally with the union of every instance's dates.
pub async fn mirror_request(state: &Arc<AppState>, raw_url: &str) -> MirrorOutcome {
    let channel = extract_value(raw_url, &CHANNEL_LINK_REGEX);
    let user = extract_value(raw_url, &USER_LINK_REGEX);

    let Some(channel) = channel else {
        return MirrorOutcome::InvalidChannel;
    };

    let data =
        crate::logs::instance::get_instance(state, &channel, user.as_deref(), false, false, None)
            .await;

    if let Some(error) = data.error {
        return MirrorOutcome::NotFound {
            status: data.status,
            message: error,
        };
    }

    let path = raw_url.split('?').next().unwrap_or(raw_url);
    let (ranked, lists) = if user.is_some() {
        (&data.user_logs.instances, &data.user_lists)
    } else {
        (&data.channel_logs.instances, &data.channel_lists)
    };

    let source = ranked.clone();

    if path.eq_ignore_ascii_case("/list") {
        let body = ListBody {
            available_logs: merge_lists(lists),
        };
        let body = match serde_json::to_vec(&body) {
            Ok(body) => body,
            Err(err) => return MirrorOutcome::UpstreamError(err.to_string()),
        };
        return MirrorOutcome::Proxied {
            status: 200,
            content_type: Some("application/json".to_string()),
            content_length: Some(body.len() as u64),
            body: Body::from(body),
            source,
        };
    }

    // A day or month that some instance has should be served from one that
    // has it, rather than 404 from whichever has the most days overall.
    let instance_link = pick_for_date(path, ranked, lists).or_else(|| ranked.first().cloned());
    let Some(instance_link) = instance_link else {
        return MirrorOutcome::NotFound {
            status: 404,
            message: "No channel logs found".to_string(),
        };
    };

    let channel_id = match &data.request.channel {
        Some(c) => c.id.clone(),
        None => {
            return MirrorOutcome::NotFound {
                status: 404,
                message: "Invalid channel".to_string(),
            };
        }
    };

    let mut request_url = CHANNEL_LINK_REGEX
        .replace(raw_url, |caps: &regex::Captures| {
            format!("channelid{}{channel_id}", &caps[1])
        })
        .into_owned();

    if user.is_some()
        && let Some(user_info) = &data.request.user
    {
        let user_id = user_info.id.clone();
        request_url = USER_LINK_REGEX
            .replace(&request_url, |caps: &regex::Captures| {
                format!("userid{}{user_id}", &caps[1])
            })
            .into_owned();
    }

    let response = state
        .http
        .get(format!("{instance_link}{request_url}"))
        .timeout(http_client::MIRROR_TIMEOUT)
        .send()
        .await;

    let response = match response {
        Ok(r) => r,
        Err(err) => return MirrorOutcome::UpstreamError(err.to_string()),
    };

    let status = response.status().as_u16();
    let content_length = response.content_length();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);

    if content_type
        .as_deref()
        .is_some_and(|ct| ct.contains("text/html"))
    {
        return MirrorOutcome::InvalidEndpoint;
    }

    MirrorOutcome::Proxied {
        status,
        content_type,
        content_length,
        // `reqwest::Body` is itself an `http_body::Body`, so the upstream
        // response pipes straight into the outgoing one without reqwest's
        // `stream` feature (and its wasm dependency tree) in the mix.
        body: Body::new(reqwest::Body::from(response)),
        source,
    }
}
