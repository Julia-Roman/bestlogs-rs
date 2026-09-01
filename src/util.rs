use regex::Regex;
use std::sync::LazyLock;

// JS `\w` is ASCII-only, so these are spelled out explicitly rather than
// using regex's (Unicode-aware) `\w`.
pub static USER_CHAN_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^[a-z0-9][a-z0-9_]{0,24}$|^id:([0-9]+)$").unwrap());

pub static USER_ID_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^id:([0-9]+)$").unwrap());

pub static TMI_SENT_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"tmi-sent-ts=(\d+)(;|\s:)").unwrap());

pub static CHANNEL_LINK_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)channel(?:id)?([/=])([a-z0-9]\w{0,24})").unwrap());

pub static USER_LINK_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)user(?:id)?([/=])([a-z0-9]\w{0,24})").unwrap());

/// Path/query params reaching here are already percent-decoded by axum's
/// extractors, so this only needs to mirror the character stripping +
/// lowercasing half of the original `formatUsername`.
pub fn format_username(input: &str) -> String {
    input
        .chars()
        .filter(|c| !matches!(c, '@' | '#' | ','))
        .collect::<String>()
        .to_lowercase()
}

pub fn is_user_or_channel(input: &str) -> bool {
    USER_CHAN_REGEX.is_match(input)
}

pub fn strip_id_prefix(input: &str) -> &str {
    input.strip_prefix("id:").unwrap_or(input)
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Matches JS `Date.prototype.toUTCString()`, e.g. "Thu, 01 Jan 1970 00:00:00 GMT".
pub fn to_utc_string(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .unwrap_or_default()
        .format("%a, %d %b %Y %H:%M:%S GMT")
        .to_string()
}
