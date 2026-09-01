use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use dashmap::DashMap;
use moka::future::Cache;

use crate::config::Config;
use crate::http_client;
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

pub struct Caches {
    /// Per-instance channel list; an empty Vec means the instance is
    /// considered down (matches the original's `instanceChannels`).
    pub instance_channels: DashMap<String, Vec<Channel>>,
    /// All channels seen across every instance, keyed by Twitch user id.
    pub unique_channels: DashMap<String, Channel>,
    pub list_data: Cache<String, Vec<AvailableLogDate>>,
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
                .max_capacity(100_000)
                .time_to_live(LIST_CACHE_TTL)
                .build(),
            status_codes: Cache::builder()
                .max_capacity(100_000)
                .time_to_live(LIST_CACHE_TTL)
                .build(),
            info_data: Cache::builder()
                .max_capacity(100_000)
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
    pub fn alive_instances(&self) -> Vec<String> {
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
            .cloned()
            .collect()
    }
}
