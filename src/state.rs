use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use dashmap::DashMap;
use moka::future::Cache;

use crate::config::Config;
use crate::http_client;
use crate::logs::channels::InstanceChannels;
use crate::logs::{AvailableLogDate, Channel};
use crate::twitch::TwitchUser;
use crate::util;

pub const RELOAD_INTERVAL_MS: i64 = 60 * 60 * 1000; // 1 hour
pub const ERROR_INTERVAL_MS: i64 = 60 * 1000; // 1 minute

/// How long a `/list` (or user-availability) probe result stays cached
/// between full reloads. Matches how long rustlog itself tells clients to
/// cache the same response (`Cache-Control: max-age=600`), so we're never
/// staler than upstream already considers acceptable. Only successful
/// probes are cached (see `logs/instance.rs`) — a timeout or 5xx isn't
/// remembered, so the very next request retries instead of being stuck
/// with a bad result for up to 10 minutes.
pub const LIST_CACHE_TTL: Duration = Duration::from_secs(600);

/// Memory budgets for the request-driven caches, in **bytes** rather than
/// entry counts.
///
/// Bounding by entry count is what let this process reach ~10 GB in
/// production: an entry's *size* is unbounded, and a `list_data` value for a
/// channel with years of daily logs is a `Vec` of a few thousand
/// `AvailableLogDate`s — hundreds of kilobytes each. Capping that at 100_000
/// entries caps nothing that matters. The key space is
/// `instance × channel`, so ~6k distinct channels across ~16 instances was
/// enough to fill it, which any crawler or Chatterino mirror client reaches
/// well inside the 10-minute TTL.
///
/// These are the tuning knobs if the cache hit rate turns out to be too low
/// on a busy deployment; the total ceiling is their sum.
const LIST_CACHE_BYTES: u64 = 256 * 1024 * 1024;
const STATUS_CACHE_BYTES: u64 = 16 * 1024 * 1024;
const INFO_CACHE_BYTES: u64 = 32 * 1024 * 1024;

/// Per-allocation overhead charged on top of payload bytes by the weighers
/// below. An `AvailableLogDate` is three separate tiny heap allocations, so
/// counting only string lengths would understate its real footprint several
/// times over and put the budgets back to being fiction.
const ALLOC_OVERHEAD: usize = 32;

fn string_weight(value: &str) -> usize {
    value.len() + ALLOC_OVERHEAD
}

fn clamp_weight(bytes: usize) -> u32 {
    bytes.min(u32::MAX as usize) as u32
}

fn list_weight(key: &str, list: &Arc<Vec<AvailableLogDate>>) -> u32 {
    let dates: usize = list
        .iter()
        .map(|date| {
            size_of::<AvailableLogDate>()
                + string_weight(&date.year)
                + string_weight(&date.month)
                + date.day.as_deref().map_or(0, string_weight)
        })
        .sum();
    clamp_weight(string_weight(key) + dates + ALLOC_OVERHEAD)
}

fn user_weight(key: &str, user: &TwitchUser) -> u32 {
    clamp_weight(
        string_weight(key)
            + size_of::<TwitchUser>()
            + string_weight(&user.name)
            + string_weight(&user.login)
            + string_weight(&user.avatar)
            + string_weight(&user.id),
    )
}

pub struct Caches {
    /// Per-instance channel list; an empty one means the instance is
    /// considered down (matches the original's `instanceChannels`).
    pub instance_channels: DashMap<String, InstanceChannels>,
    /// All channels seen across every instance, keyed by Twitch user id.
    pub unique_channels: DashMap<String, Channel>,
    /// Behind an `Arc` so the ~16 per-request copies of a channel's date
    /// list (one per probed instance, plus the ranked/response copies) are
    /// refcount bumps instead of deep clones of a few thousand `String`s.
    pub list_data: Cache<String, Arc<Vec<AvailableLogDate>>>,
    pub status_codes: Cache<String, u16>,
    /// Unlike the original (which never expires this cache), a TTL is
    /// applied here to avoid unbounded growth over long uptimes.
    pub info_data: Cache<String, TwitchUser>,
}

impl Caches {
    fn new() -> Caches {
        Caches {
            instance_channels: DashMap::new(),
            unique_channels: DashMap::new(),
            list_data: Cache::builder()
                .max_capacity(LIST_CACHE_BYTES)
                .weigher(|key: &String, list: &Arc<Vec<AvailableLogDate>>| list_weight(key, list))
                .time_to_live(LIST_CACHE_TTL)
                .build(),
            status_codes: Cache::builder()
                .max_capacity(STATUS_CACHE_BYTES)
                .weigher(|key: &String, _: &u16| {
                    clamp_weight(string_weight(key) + size_of::<u16>())
                })
                .time_to_live(LIST_CACHE_TTL)
                .build(),
            info_data: Cache::builder()
                .max_capacity(INFO_CACHE_BYTES)
                .weigher(|key: &String, user: &TwitchUser| user_weight(key, user))
                .time_to_live(Duration::from_secs(60 * 60))
                .build(),
        }
    }

    /// Matches `loadInstanceChannels`'s full-reload behaviour of clearing
    /// the derived list/status caches once channel lists are refreshed.
    pub fn clear_derived(&self) {
        self.list_data.invalidate_all();
        self.status_codes.invalidate_all();
    }

    /// Port of `checkInstances`: counts entries currently tracked, and how
    /// many are flagged down (an empty channel list).
    pub fn stats(&self) -> (usize, usize) {
        let count = self.instance_channels.len();
        let down = self
            .instance_channels
            .iter()
            .filter(|e| e.value().is_empty())
            .count();
        (count, down)
    }
}

pub struct AppState {
    pub config: Config,
    pub http: reqwest::Client,
    pub version: String,
    pub commit: String,
    pub caches: Caches,
    pub last_updated_ms: AtomicI64,
    pub started_at_ms: i64,
}

impl AppState {
    pub fn new(config: Config) -> AppState {
        AppState {
            config,
            http: http_client::build_client(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            commit: env!("GIT_COMMIT_HASH").to_string(),
            caches: Caches::new(),
            last_updated_ms: AtomicI64::new(util::now_ms()),
            started_at_ms: util::now_ms(),
        }
    }

    pub fn last_updated_ms(&self) -> i64 {
        self.last_updated_ms.load(Ordering::Relaxed)
    }

    pub fn mark_updated(&self) {
        self.last_updated_ms
            .store(util::now_ms(), Ordering::Relaxed);
    }

    /// The hostname actually queried for an instance: its configured
    /// `alternate`, or the instance key itself.
    pub fn instance_host(&self, key: &str) -> String {
        self.config
            .justlogs_instances
            .get(key)
            .and_then(|meta| meta.alternate.clone())
            .unwrap_or_else(|| key.to_string())
    }

    /// Instances not currently flagged as down (i.e. with a non-empty last
    /// loaded channel list, or not loaded yet).
    ///
    /// Borrowed from the config rather than cloned: this runs once per
    /// lookup and the keys outlive any single request.
    pub fn alive_instances(&self) -> Vec<&str> {
        self.config
            .justlogs_instances
            .keys()
            .filter(|key| {
                self.caches
                    .instance_channels
                    .get(*key)
                    .map(|entry| !entry.is_empty())
                    .unwrap_or(true)
            })
            .map(String::as_str)
            .collect()
    }
}
