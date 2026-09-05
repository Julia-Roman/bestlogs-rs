//! Load-generator for the log fan-out (`logs::instance::get_instance`).
//!
//! Spins up a set of local TLS mock instances with latency profiles modelled
//! on the real justlog/rustlog deployments (a few fast, several middling, a
//! couple slow, one that spikes, one flaky, one black hole), points a real
//! `AppState` at them, and drives concurrent lookups through the actual
//! ranking path — the Twitch resolution cache is pre-seeded so the only
//! network in the picture is the fan-out itself.
//!
//! Run with:
//!   cargo run --release --features bench --bin fanout-bench -- <cert.der> <key.der>

// The target is `required-features = ["bench"]`, so cargo never builds this
// file without the feature — but rust-analyzer still analyses it, and without
// the gate it reports every `tokio_rustls` import as unresolved. With the gate
// the file simply reads as inactive until the feature is on.
#![cfg(feature = "bench")]
#![allow(dead_code)]

#[path = "../config.rs"]
mod config;
#[path = "../http_client.rs"]
mod http_client;
#[path = "../logs/mod.rs"]
mod logs;
#[path = "../reload.rs"]
mod reload;
#[path = "../state.rs"]
mod state;
#[path = "../twitch.rs"]
mod twitch;
#[path = "../util.rs"]
mod util;

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::TlsAcceptor;
use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer};

use crate::config::{Config, InstanceMeta, JustlogInstanceMeta, RateLimitConfig};
use crate::logs::instance::get_instance;
use crate::logs::{Channel, channels::InstanceChannels};
use crate::state::AppState;
use crate::twitch::TwitchUser;

const CHANNELS: usize = 4_000;
/// Share of lookups drawn from the hot 5% of channels; the rest are spread
/// uniformly, which is what keeps the probe caches realistically missing.
const HOT_SHARE: u64 = 60;
const USER_LOOKUP_SHARE: u64 = 35;
/// Share of instances that carry any given channel.
const PRESENCE: u64 = 65;

#[derive(Clone, Copy)]
struct Profile {
    name: &'static str,
    base_ms: u64,
    jitter_ms: u64,
    spike_pct: u64,
    spike_ms: u64,
    error_pct: u64,
    blackhole: bool,
}

/// In the `recovery` scenario, the `flappy` instance black-holes everything
/// for this long and then serves normally, which is what exercises the
/// breaker's trip *and* close paths rather than just the trip.
const OUTAGE_WINDOW: Duration = Duration::from_secs(15);

const fn profile(
    name: &'static str,
    base_ms: u64,
    jitter_ms: u64,
    spike_pct: u64,
    spike_ms: u64,
    error_pct: u64,
    blackhole: bool,
) -> Profile {
    Profile {
        name,
        base_ms,
        jitter_ms,
        spike_pct,
        spike_ms,
        error_pct,
        blackhole,
    }
}

/// Sixteen instances, the same count the shipped config carries.
const PROFILES: &[Profile] = &[
    profile("fast-1", 25, 20, 0, 0, 0, false),
    profile("fast-2", 30, 20, 0, 0, 0, false),
    profile("fast-3", 35, 25, 0, 0, 0, false),
    profile("fast-4", 40, 30, 0, 0, 0, false),
    profile("fast-5", 45, 30, 0, 0, 0, false),
    profile("fast-6", 55, 40, 0, 0, 0, false),
    profile("mid-1", 120, 80, 0, 0, 0, false),
    profile("mid-2", 150, 90, 0, 0, 0, false),
    profile("mid-3", 180, 100, 0, 0, 0, false),
    profile("mid-4", 220, 120, 0, 0, 0, false),
    profile("mid-5", 260, 140, 0, 0, 0, false),
    profile("slow-1", 800, 400, 0, 0, 0, false),
    profile("slow-2", 1100, 500, 0, 0, 0, false),
    profile("spiky", 250, 100, 15, 4_000, 0, false),
    profile("flaky", 200, 100, 0, 0, 20, false),
    profile("blackhole", 0, 0, 0, 0, 0, true),
    profile("flappy", 60, 40, 0, 0, 0, false),
];

fn hash(parts: &[&str], salt: u64) -> u64 {
    let mut value: u64 = 0xcbf2_9ce4_8422_2325 ^ salt;
    for part in parts {
        for byte in part.as_bytes() {
            value ^= *byte as u64;
            value = value.wrapping_mul(0x1000_0000_01b3);
        }
        value = value.wrapping_mul(0x9e37_79b9_7f4a_7c15);
    }
    value
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, bound: u64) -> u64 {
        if bound == 0 { 0 } else { self.next() % bound }
    }
}

/// `/list` bodies, keyed by day count — the mocks only vary in how many days
/// they report, so one body per length is all that's ever needed.
fn list_body(days: usize) -> Arc<String> {
    static BODIES: OnceLock<Mutex<HashMap<usize, Arc<String>>>> = OnceLock::new();
    let bodies = BODIES.get_or_init(|| Mutex::new(HashMap::new()));

    if let Some(body) = bodies.lock().unwrap().get(&days) {
        return body.clone();
    }

    let mut json = String::from("{\"availableLogs\":[");
    for day in 0..days {
        if day > 0 {
            json.push(',');
        }
        let year = 2020 + (day / 365);
        let month = 1 + (day / 28) % 12;
        json.push_str(&format!(
            "{{\"year\":\"{year}\",\"month\":\"{month}\",\"day\":\"{}\"}}",
            1 + day % 28
        ));
    }
    json.push_str("]}");

    let body = Arc::new(json);
    bodies.lock().unwrap().insert(days, body.clone());
    body
}

#[derive(Default)]
struct MockStats {
    requests: AtomicU64,
    errors: AtomicU64,
}

async fn read_request(stream: &mut tokio_rustls::server::TlsStream<TcpStream>) -> Option<String> {
    let mut head = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        let read = stream.read(&mut chunk).await.ok()?;
        if read == 0 {
            return None;
        }
        head.extend_from_slice(&chunk[..read]);
        if head.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
    }
    let text = String::from_utf8_lossy(&head).into_owned();
    text.lines()
        .next()
        .map(|line| line.split_whitespace().nth(1).unwrap_or("/").to_string())
}

fn query_value<'a>(query: &'a str, keys: &[&str]) -> Option<&'a str> {
    query.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        keys.contains(&key).then_some(value)
    })
}

async fn serve_mock(
    listener: TcpListener,
    acceptor: TlsAcceptor,
    profile: Profile,
    index: usize,
    stats: Arc<MockStats>,
    outage_until: Option<Instant>,
) {
    loop {
        let Ok((socket, _)) = listener.accept().await else {
            continue;
        };
        let acceptor = acceptor.clone();
        let stats = stats.clone();
        tokio::spawn(async move {
            let in_outage = || outage_until.is_some_and(|until| Instant::now() < until);
            let Ok(mut stream) = acceptor.accept(socket).await else {
                return;
            };
            let mut rng = Rng(hash(&[profile.name], stats.requests.load(Ordering::Relaxed)) | 1);

            while let Some(path) = read_request(&mut stream).await {
                stats.requests.fetch_add(1, Ordering::Relaxed);

                if profile.blackhole || in_outage() {
                    tokio::time::sleep(Duration::from_secs(30)).await;
                    return;
                }

                let query = path.split_once('?').map(|(_, q)| q).unwrap_or("");
                let channel = query_value(query, &["channel", "channelid"]).unwrap_or("unknown");
                let user = query_value(query, &["user", "userid"]);

                if profile.error_pct > 0 && rng.below(100) < profile.error_pct {
                    stats.errors.fetch_add(1, Ordering::Relaxed);
                    let _ = stream
                        .write_all(
                            b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: keep-alive\r\n\r\n",
                        )
                        .await;
                    continue;
                }

                let mut latency = profile.base_ms + rng.below(profile.jitter_ms.max(1));
                if profile.spike_pct > 0 && rng.below(100) < profile.spike_pct {
                    latency += profile.spike_ms;
                }
                tokio::time::sleep(Duration::from_millis(latency)).await;

                let response = match user {
                    Some(user) => {
                        let roll = hash(&[channel, user], index as u64) % 100;
                        let status = if roll < 70 {
                            "200 OK"
                        } else if roll < 90 {
                            "404 Not Found"
                        } else {
                            "403 Forbidden"
                        };
                        format!(
                            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: keep-alive\r\n\r\n{{}}"
                        )
                    }
                    None => {
                        let days = 30 + (hash(&[channel], index as u64) % 900) as usize;
                        let body = list_body(days);
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: keep-alive\r\n\r\n{body}",
                            body.len()
                        )
                    }
                };

                if stream.write_all(response.as_bytes()).await.is_err() {
                    return;
                }
            }
        });
    }
}

fn tls_acceptor(cert_path: &str, key_path: &str) -> TlsAcceptor {
    let cert = CertificateDer::from(std::fs::read(cert_path).expect("cert.der"));
    let key = PrivateKeyDer::try_from(std::fs::read(key_path).expect("key.der")).expect("key");

    let mut config = tokio_rustls::rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert], key)
        .expect("server config");
    // Keep the mocks on HTTP/1.1 so one slow response can't be multiplexed
    // behind another and blur the profiles.
    config.alpn_protocols = vec![b"http/1.1".to_vec()];

    TlsAcceptor::from(Arc::new(config))
}

struct Sample {
    at: Duration,
    latency: Duration,
    contributing: usize,
    down: usize,
    status: u16,
    /// Days reported by the instance that came out on top, over the most any
    /// answerable instance holds for that channel. 1.0 means the lookup
    /// ranked the genuinely best instance first.
    rank_quality: f64,
}

fn percentile(sorted: &[Duration], pct: f64) -> Duration {
    if sorted.is_empty() {
        return Duration::ZERO;
    }
    let index = ((sorted.len() - 1) as f64 * pct / 100.0).round() as usize;
    sorted[index]
}

fn report(label: &str, samples: &[Sample], elapsed: Duration, mocks: &[(String, Arc<MockStats>)]) {
    let mut latencies: Vec<Duration> = samples.iter().map(|s| s.latency).collect();
    latencies.sort_unstable();

    let contributing: f64 =
        samples.iter().map(|s| s.contributing as f64).sum::<f64>() / samples.len().max(1) as f64;
    let down: f64 =
        samples.iter().map(|s| s.down as f64).sum::<f64>() / samples.len().max(1) as f64;
    let non_200 = samples.iter().filter(|s| s.status != 200).count();
    let quality: f64 =
        samples.iter().map(|s| s.rank_quality).sum::<f64>() / samples.len().max(1) as f64;
    let ideal = samples.iter().filter(|s| s.rank_quality >= 1.0).count();

    println!("\n=== {label} ===");
    println!(
        "requests {} over {:.1}s ({:.0} req/s)",
        samples.len(),
        elapsed.as_secs_f64(),
        samples.len() as f64 / elapsed.as_secs_f64()
    );
    for (name, pct) in [("p50", 50.0), ("p90", 90.0), ("p99", 99.0)] {
        println!(
            "{name:>6}: {:>8.0}ms",
            percentile(&latencies, pct).as_secs_f64() * 1000.0
        );
    }
    println!(
        "   max: {:>8.0}ms",
        latencies.last().copied().unwrap_or_default().as_secs_f64() * 1000.0
    );
    println!("instances ranked per lookup: {contributing:.2} | reported down: {down:.2}");
    println!("non-200 answers: {non_200}");
    println!(
        "ranking quality: {:.3} mean, best instance ranked first on {:.1}% of lookups",
        quality,
        100.0 * ideal as f64 / samples.len().max(1) as f64
    );

    println!("per-5s-window p50 / p99 (ms):");
    let windows = (elapsed.as_secs() / 5).max(1);
    for window in 0..windows {
        let lo = Duration::from_secs(window * 5);
        let hi = Duration::from_secs((window + 1) * 5);
        let mut bucket: Vec<Duration> = samples
            .iter()
            .filter(|s| s.at >= lo && s.at < hi)
            .map(|s| s.latency)
            .collect();
        bucket.sort_unstable();
        let ranked: f64 = samples
            .iter()
            .filter(|s| s.at >= lo && s.at < hi)
            .map(|s| s.contributing as f64)
            .sum::<f64>()
            / bucket.len().max(1) as f64;
        println!(
            "  {:>3}-{:>3}s  n={:<6} {:>7.0} / {:>7.0}   ranked {ranked:.2}",
            lo.as_secs(),
            hi.as_secs(),
            bucket.len(),
            percentile(&bucket, 50.0).as_secs_f64() * 1000.0,
            percentile(&bucket, 99.0).as_secs_f64() * 1000.0
        );
    }

    println!("upstream probes served per instance:");
    for (name, stats) in mocks {
        println!(
            "  {name:<12} {:>7} ({} errors)",
            stats.requests.load(Ordering::Relaxed),
            stats.errors.load(Ordering::Relaxed)
        );
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .init();

    let mut args = std::env::args().skip(1);
    let cert = args
        .next()
        .expect("usage: fanout-bench <cert.der> <key.der>");
    let key = args
        .next()
        .expect("usage: fanout-bench <cert.der> <key.der>");
    let label = std::env::var("BENCH_LABEL").unwrap_or_else(|_| "run".to_string());
    let seconds: u64 = std::env::var("BENCH_SECONDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(30);
    let concurrency: usize = std::env::var("BENCH_CONCURRENCY")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(32);

    // Which slice of the profile list to stand up. `healthy` is the control
    // (nothing is misbehaving), `degraded` is the ordinary day where a couple
    // of instances are slow or flaky, `outage` adds a host that accepts
    // connections and then never answers.
    let scenario = std::env::var("BENCH_SCENARIO").unwrap_or_else(|_| "outage".to_string());
    let profiles: Vec<Profile> = PROFILES
        .iter()
        .copied()
        .filter(|profile| match scenario.as_str() {
            "healthy" => profile.name.starts_with("fast") || profile.name.starts_with("mid"),
            "degraded" => !profile.blackhole && profile.name != "flappy",
            "recovery" => profile.name.starts_with("fast") || profile.name == "flappy",
            _ => profile.name != "flappy",
        })
        .collect();

    let acceptor = tls_acceptor(&cert, &key);
    let mut instances = indexmap::IndexMap::new();
    let mut mocks = Vec::new();

    for (index, profile) in profiles.iter().enumerate() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = listener.local_addr().unwrap().port();
        let stats = Arc::new(MockStats::default());
        tokio::spawn(serve_mock(
            listener,
            acceptor.clone(),
            *profile,
            index,
            stats.clone(),
            (profile.name == "flappy").then(|| Instant::now() + OUTAGE_WINDOW),
        ));

        instances.insert(
            format!("{}.bench", profile.name),
            JustlogInstanceMeta {
                maintainer: Some("bench".to_string()),
                alternate: Some(format!("127.0.0.1:{port}")),
            },
        );
        mocks.push((profile.name.to_string(), stats));
    }

    let config = Config {
        port: 0,
        instance: InstanceMeta::default(),
        justlogs_instances: instances,
        recentmessages_instances: indexmap::IndexMap::new(),
        rate_limit: RateLimitConfig::default(),
        umami_stats: None,
    };
    let state = Arc::new(AppState::new(config));

    // Channel lists and Twitch resolution are what `reload.rs` and ivr.fi
    // would provide; seeded directly so the benchmark exercises the fan-out
    // and nothing else.
    let logins: Vec<String> = (0..CHANNELS).map(|i| format!("chan{i}")).collect();
    for key in state.config.justlogs_instances.keys() {
        let channels: Vec<Channel> = logins
            .iter()
            .enumerate()
            .filter(|(id, _)| hash(&[key], *id as u64) % 100 < PRESENCE)
            .map(|(id, name)| Channel {
                name: name.clone(),
                user_id: (1_000_000 + id).to_string(),
            })
            .collect();
        state
            .caches
            .instance_channels
            .insert(key.clone(), InstanceChannels::new(channels));
    }
    for (id, login) in logins.iter().enumerate() {
        state
            .caches
            .info_data
            .insert(
                login.clone(),
                TwitchUser {
                    name: login.clone(),
                    login: login.clone(),
                    avatar: String::new(),
                    id: (1_000_000 + id).to_string(),
                    banned: false,
                },
            )
            .await;
    }

    println!(
        "[{label}] scenario {scenario}: {} instances, {concurrency} concurrent clients, {seconds}s, {CHANNELS} channels",
        profiles.len()
    );

    // The mocks derive their day count from `hash(channel, instance index)`,
    // so the driver can compute what a perfect ranking would have returned
    // and score each answer against it. Instances that never respond at all
    // are excluded: no strategy can rank what does not reply.
    let answerable: Arc<Vec<(String, usize)>> = Arc::new(
        state
            .config
            .justlogs_instances
            .keys()
            .enumerate()
            .filter(|(index, _)| !profiles[*index].blackhole)
            .map(|(index, key)| (key.clone(), index))
            .collect(),
    );

    let started = Instant::now();
    let deadline = started + Duration::from_secs(seconds);
    let mut workers = Vec::new();

    for worker in 0..concurrency {
        let state = state.clone();
        let logins = logins.clone();
        let answerable = answerable.clone();
        workers.push(tokio::spawn(async move {
            let mut rng = Rng(0x243f_6a88_85a3_08d3 ^ (worker as u64 + 1).wrapping_mul(0x9e37));
            let mut samples = Vec::new();

            while Instant::now() < deadline {
                // Hot channels dominate (a live streamer everyone is looking
                // up) with a long uniform tail, so the probe caches see a
                // realistic mix of hits and misses.
                let picked = if rng.below(100) < HOT_SHARE {
                    rng.below((CHANNELS / 20) as u64) as usize
                } else {
                    rng.below(CHANNELS as u64) as usize
                };
                let channel = &logins[picked];
                let user = (rng.below(100) < USER_LOOKUP_SHARE)
                    .then(|| logins[rng.below(CHANNELS as u64) as usize].clone());

                let best_days = answerable
                    .iter()
                    .filter(|(key, _)| hash(&[key], picked as u64) % 100 < PRESENCE)
                    .map(|(_, index)| 30 + hash(&[channel], *index as u64) % 900)
                    .max()
                    .unwrap_or(0) as f64;

                let at = started.elapsed();
                let call = Instant::now();
                let result =
                    get_instance(&state, channel, user.as_deref(), false, false, None).await;
                samples.push(Sample {
                    at,
                    latency: call.elapsed(),
                    contributing: result.channel_logs.count,
                    down: result.instances_info.down,
                    status: result.status,
                    rank_quality: if best_days > 0.0 {
                        result.logged_data.days as f64 / best_days
                    } else {
                        1.0
                    },
                });
            }

            samples
        }));
    }

    let mut samples = Vec::new();
    for worker in workers {
        samples.extend(worker.await.expect("worker"));
    }

    report(&label, &samples, started.elapsed(), &mocks);
}
