use std::io;
use std::time::Duration;

use reqwest::dns::{Addrs, Name, Resolve, Resolving};

pub const USER_AGENT: &str = "Best Logs by ZonianMidian";

/// Several hobbyist-run justlog/rustlog instances use self-signed certs; the
/// original explicitly disabled certificate verification (`rejectUnauthorized:
/// false`) for exactly this reason, so this port does the same.
///
/// One shared client for the whole process: every lookup, mirror proxy, and
/// background reload hits the same ~17 configured hosts repeatedly, so
/// connection reuse (keep-alive, TLS session resumption) meaningfully cuts
/// latency instead of paying a fresh TCP+TLS handshake per request.
pub fn build_client(force_ipv4: bool) -> reqwest::Client {
    let mut builder = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .user_agent(USER_AGENT)
        .tcp_keepalive(Duration::from_secs(60))
        .pool_idle_timeout(Duration::from_secs(90));
    if force_ipv4 {
        builder = builder.dns_resolver(Ipv4OnlyResolver);
    }
    builder.build().expect("failed to build reqwest client")
}

/// Filtering at resolution time rather than via `local_address(0.0.0.0)`:
/// hyper only applies a local address whose family matches the remote one,
/// so binding to an IPv4 address still lets IPv6 connections through.
struct Ipv4OnlyResolver;

impl Resolve for Ipv4OnlyResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let host = name.as_str().to_owned();
        Box::pin(async move {
            let addrs: Vec<_> = tokio::net::lookup_host((host.as_str(), 0))
                .await?
                .filter(|addr| addr.is_ipv4())
                .collect();
            if addrs.is_empty() {
                return Err(io::Error::new(
                    io::ErrorKind::AddrNotAvailable,
                    format!("{host} has no IPv4 address"),
                )
                .into());
            }
            Ok(Box::new(addrs.into_iter()) as Addrs)
        })
    }
}

pub const LIST_TIMEOUT: Duration = Duration::from_secs(5);
pub const RELOAD_TIMEOUT: Duration = Duration::from_secs(10);
pub const MIRROR_TIMEOUT: Duration = Duration::from_secs(120);
