use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use moka::future::Cache;

use crate::config::RateLimitConfig;

/// Ceiling on how many distinct source addresses are tracked at once, so a
/// spray of one request per forged/rotating IP can't grow the limiter
/// without bound. Each entry is a couple of dozen bytes; evicting the
/// least-recently-used one just means that address starts from a full
/// bucket again, which is exactly what it would have got by waiting.
const MAX_TRACKED_ADDRESSES: u64 = 100_000;

struct Bucket {
    tokens: f64,
    updated: Instant,
}

pub struct RateLimiter {
    capacity: f64,
    refill_per_second: f64,
    trust_proxy: bool,
    buckets: Cache<IpAddr, Arc<Mutex<Bucket>>>,
}

impl RateLimiter {
    /// `None` when rate limiting is switched off (or configured with a zero
    /// budget, which would otherwise reject every request); callers then
    /// skip installing the middleware entirely.
    pub fn new(config: &RateLimitConfig) -> Option<Arc<RateLimiter>> {
        if !config.enabled || config.events == 0 || config.interval_seconds == 0 {
            return None;
        }

        let interval = Duration::from_secs(config.interval_seconds);

        Some(Arc::new(RateLimiter {
            capacity: f64::from(config.events),
            refill_per_second: f64::from(config.events) / interval.as_secs_f64(),
            trust_proxy: config.trust_proxy,
            buckets: Cache::builder()
                .max_capacity(MAX_TRACKED_ADDRESSES)
                // A bucket untouched for a full interval has refilled to
                // capacity, so forgetting it is indistinguishable from
                // keeping it.
                .time_to_idle(interval)
                .build(),
        }))
    }

    /// `Err` carries how long the caller should wait before retrying.
    async fn check(&self, address: IpAddr) -> Result<(), Duration> {
        let bucket = self
            .buckets
            .get_with(address, async {
                Arc::new(Mutex::new(Bucket {
                    tokens: self.capacity,
                    updated: Instant::now(),
                }))
            })
            .await;

        let mut bucket = bucket.lock().unwrap_or_else(|err| err.into_inner());

        let now = Instant::now();
        let elapsed = now.duration_since(bucket.updated).as_secs_f64();
        bucket.updated = now;
        bucket.tokens = (bucket.tokens + elapsed * self.refill_per_second).min(self.capacity);

        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            Ok(())
        } else {
            let seconds = (1.0 - bucket.tokens) / self.refill_per_second;
            Err(Duration::from_secs(seconds.ceil().max(1.0) as u64))
        }
    }

    /// With `trust_proxy` off, only the peer address counts — client-supplied
    /// forwarding headers are trivially forged, so honouring them on a
    /// directly-exposed deployment would hand every client its own bucket.
    fn client_address(&self, headers: &HeaderMap, peer: SocketAddr) -> IpAddr {
        if !self.trust_proxy {
            return peer.ip();
        }

        forwarded_address(headers).unwrap_or_else(|| peer.ip())
    }
}

fn forwarded_address(headers: &HeaderMap) -> Option<IpAddr> {
    let header = |name: &str| headers.get(name).and_then(|value| value.to_str().ok());

    let candidate = header("x-forwarded-for")
        .or_else(|| header("x-real-ip"))
        .or_else(|| header("cf-connecting-ip").and_then(|chain| chain.split(',').next()))?;

    candidate.trim().parse().ok()
}

fn too_many_requests(retry_after: Duration) -> Response {
    let mut response = (StatusCode::TOO_MANY_REQUESTS, "Too Many Requests").into_response();
    response
        .headers_mut()
        .insert(header::RETRY_AFTER, retry_after.as_secs().into());
    response
}

pub async fn limit(
    State(limiter): State<Arc<RateLimiter>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    request: Request,
    next: Next,
) -> Response {
    let address = limiter.client_address(&headers, peer);

    match limiter.check(address).await {
        Ok(()) => next.run(request).await,
        Err(retry_after) => too_many_requests(retry_after),
    }
}
