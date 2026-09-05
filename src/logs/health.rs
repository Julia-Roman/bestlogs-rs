use std::time::{Duration, Instant};

use dashmap::DashMap;
use tracing::{info, warn};

use crate::http_client::LIST_TIMEOUT;

/// Consecutive failed probes before an instance is dropped from the fan-out.
const FAILURE_THRESHOLD: u32 = 3;

/// How long a tripped instance is skipped for, doubling on each further
/// failed trial so a host that has been down for an hour is retried once a
/// few minutes rather than by every request that arrives.
const BASE_COOLDOWN: Duration = Duration::from_secs(15);
const MAX_COOLDOWN: Duration = Duration::from_secs(300);

/// Floor for the adaptive timeout: an instance that normally answers in 20ms
/// still gets a fair chance at an unusually cold `/list`.
const MIN_TIMEOUT: Duration = Duration::from_millis(750);

/// Timeout granted to a healthy instance, as a multiple of its smoothed
/// response time (clamped to `MIN_TIMEOUT..=LIST_TIMEOUT`).
const TIMEOUT_FACTOR: f64 = 4.0;

/// EWMA smoothing for the observed response time. High enough that a single
/// slow response widens the timeout immediately, low enough that one fast
/// response after a bad spell doesn't.
const ALPHA: f64 = 0.25;

/// How long a claimed half-open trial is honoured before another request may
/// claim it. The claimant is not guaranteed to report back — its probe can be
/// answered from the cache without ever reaching the network — so the claim
/// has to expire on its own, or one such request would wedge the breaker open
/// and the instance would never be retried. Comfortably longer than the
/// slowest probe a trial can make.
const TRIAL_TIMEOUT: Duration = Duration::from_secs(10);

/// What a caller is allowed to do with an instance right now.
#[derive(Clone, Copy)]
pub enum Permit {
    /// Query it, giving up after this long.
    Fetch(Duration),
    /// The instance is currently tripped: use whatever is already cached for
    /// it, but do not put a request on the wire.
    CachedOnly,
}

#[derive(Default)]
struct Entry {
    /// Smoothed response time of successful probes, in milliseconds.
    ewma_ms: Option<f64>,
    consecutive_failures: u32,
    /// Number of times the breaker has tripped without a success since, used
    /// as the exponent for the cooldown backoff.
    trips: u32,
    /// Set while the breaker is tripped. Once it is in the past the instance
    /// is half-open: exactly one request is let through as a trial.
    open_until: Option<Instant>,
    /// Deadline of the outstanding half-open trial, if one is claimed.
    trial_until: Option<Instant>,
}

impl Entry {
    fn timeout(&self) -> Duration {
        match self.ewma_ms {
            Some(ms) => Duration::from_secs_f64(ms * TIMEOUT_FACTOR / 1000.0)
                .clamp(MIN_TIMEOUT, LIST_TIMEOUT),
            None => LIST_TIMEOUT,
        }
    }

    fn cooldown(&self) -> Duration {
        BASE_COOLDOWN
            .saturating_mul(1u32 << self.trips.min(5))
            .min(MAX_COOLDOWN)
    }
}

/// Per-host request-path health, shared by the log fan-out and the
/// recent-messages lookup.
///
/// Both of those query a fixed set of third-party hosts that are run by
/// hobbyists and go down, get overloaded, or start black-holing requests
/// without ever failing outright. Two things follow from that:
///
/// * A host that has failed several times in a row is very likely to fail
///   again, and waiting out a full timeout on it once per request (times
///   every concurrent request) is pure added latency — so it is skipped
///   entirely until a cooldown expires, then retried with a single trial
///   request rather than the whole incoming load at once.
/// * A host's normal response time is a much better bound than one global
///   timeout: 5s is far too generous for a host that always answers in 40ms
///   and about right for one that is genuinely slow but useful.
pub struct InstanceHealth {
    entries: DashMap<String, Entry>,
}

impl InstanceHealth {
    pub fn new() -> InstanceHealth {
        InstanceHealth {
            entries: DashMap::new(),
        }
    }

    /// Decides whether `host` may be queried, and how long to wait on it.
    ///
    /// Claims the half-open trial slot as a side effect, so a tripped
    /// instance is retried by one request at a time instead of by all of
    /// them the moment its cooldown expires.
    pub fn permit(&self, host: &str) -> Permit {
        let now = Instant::now();
        let mut entry = self.entries.entry(host.to_string()).or_default();

        match entry.open_until {
            Some(until) if now < until => Permit::CachedOnly,
            Some(_) if entry.trial_until.is_some_and(|until| now < until) => Permit::CachedOnly,
            Some(_) => {
                entry.trial_until = Some(now + TRIAL_TIMEOUT);
                Permit::Fetch(entry.timeout())
            }
            None => Permit::Fetch(entry.timeout()),
        }
    }

    pub fn record_success(&self, host: &str, elapsed: Duration) {
        let mut entry = self.entries.entry(host.to_string()).or_default();
        let ms = elapsed.as_secs_f64() * 1000.0;

        entry.ewma_ms = Some(match entry.ewma_ms {
            Some(previous) => previous * (1.0 - ALPHA) + ms * ALPHA,
            None => ms,
        });
        entry.consecutive_failures = 0;
        entry.trial_until = None;
        if entry.open_until.take().is_some() {
            entry.trips = 0;
            info!("[{host}] Responding again ({ms:.0}ms); back in the fan-out");
        }
    }

    pub fn record_failure(&self, host: &str, reason: &str) {
        let now = Instant::now();
        let mut entry = self.entries.entry(host.to_string()).or_default();

        entry.consecutive_failures += 1;
        entry.trial_until = None;

        // Already tripped, so this is one of the probes that was still in
        // flight when it happened. A fan-out has a dozen of those, and letting
        // each one extend the cooldown took a single burst of concurrent
        // failures straight to the maximum — the instance then sat out five
        // minutes over one bad second. Only a failed trial escalates.
        if entry.open_until.is_some_and(|until| now < until) {
            return;
        }

        if entry.consecutive_failures < FAILURE_THRESHOLD {
            return;
        }

        let cooldown = entry.cooldown();
        entry.open_until = Some(now + cooldown);
        entry.trips += 1;
        warn!(
            "[{host}] Skipping for {}s after {} failed probes: {reason}",
            cooldown.as_secs(),
            entry.consecutive_failures
        );
    }
}
