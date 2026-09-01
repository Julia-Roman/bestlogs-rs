use std::time::Duration;

pub const USER_AGENT: &str = "Best Logs by ZonianMidian";

/// Several hobbyist-run justlog/rustlog instances use self-signed certs; the
/// original explicitly disabled certificate verification (`rejectUnauthorized:
/// false`) for exactly this reason, so this port does the same.
///
/// One shared client for the whole process: every lookup, mirror proxy, and
/// background reload hits the same ~17 configured hosts repeatedly, so
/// connection reuse (keep-alive, TLS session resumption) meaningfully cuts
/// latency instead of paying a fresh TCP+TLS handshake per request.
pub fn build_client() -> reqwest::Client {
    reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .user_agent(USER_AGENT)
        .tcp_keepalive(Duration::from_secs(60))
        .pool_idle_timeout(Duration::from_secs(90))
        .build()
        .expect("failed to build reqwest client")
}

pub const LIST_TIMEOUT: Duration = Duration::from_secs(5);
pub const RELOAD_TIMEOUT: Duration = Duration::from_secs(10);
pub const MIRROR_TIMEOUT: Duration = Duration::from_secs(120);
