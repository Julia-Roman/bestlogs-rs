use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use tracing::{error, info};

use crate::http_client;
use crate::logs::Channel;
use crate::logs::channels::InstanceChannels;
use crate::state::{AppState, ERROR_INTERVAL_MS, RELOAD_INTERVAL_MS};

#[derive(Deserialize)]
struct ChannelsResponse {
    #[serde(default)]
    channels: Vec<Channel>,
}

/// Port of `Utils.loadInstanceChannels`: refreshes each justlog/rustlog
/// instance's channel list. With `only_error` set, only instances currently
/// flagged down are rechecked, and the shared "last updated" state / derived
/// caches are left untouched (matching the original's error-recheck pass).
pub async fn load_instance_channels(state: &Arc<AppState>, only_error: bool) {
    let instances: Vec<String> = if only_error {
        state
            .config
            .justlogs_instances
            .keys()
            .filter(|key| {
                state
                    .caches
                    .instance_channels
                    .get(*key)
                    .map(|entry| entry.is_empty())
                    .unwrap_or(false)
            })
            .cloned()
            .collect()
    } else {
        state.config.justlogs_instances.keys().cloned().collect()
    };

    if instances.is_empty() && !only_error {
        info!("[Logs] No instances found");
        return;
    }

    let mut working = 0usize;
    let results = futures::future::join_all(instances.iter().map(|key| {
        let state = state.clone();
        let key = key.clone();
        async move {
            let host = state.instance_host(&key);
            let result = state
                .http
                .get(format!("https://{host}/channels"))
                .timeout(http_client::RELOAD_TIMEOUT)
                .send()
                .await;

            let outcome = async {
                let response = result?.error_for_status()?;
                let body: ChannelsResponse = response.json().await?;
                if body.channels.is_empty() {
                    anyhow::bail!("No channels found");
                }
                Ok::<_, anyhow::Error>(body.channels)
            }
            .await;

            (key, outcome)
        }
    }))
    .await;

    for (key, outcome) in results {
        match outcome {
            Ok(channels) => {
                for channel in &channels {
                    state
                        .caches
                        .unique_channels
                        .insert(channel.user_id.clone(), channel.clone());
                }
                if !only_error {
                    info!("[{key}] Loaded {} channels", channels.len());
                }
                working += 1;
                state
                    .caches
                    .instance_channels
                    .insert(key, InstanceChannels::new(channels));
            }
            Err(err) => {
                if !only_error {
                    error!("[{key}] Failed loading channels: {err}");
                }
                state
                    .caches
                    .instance_channels
                    .insert(key, InstanceChannels::empty());
            }
        }
    }

    if !only_error {
        state.mark_updated();
        state.caches.clear_derived();
        info!(
            "[Logs] Loaded {} unique channels from {}/{} instances",
            state.caches.unique_channels.len(),
            working,
            state.caches.instance_channels.len()
        );
    }
}

/// Spawns the two background refresh loops, matching
/// `loopLoadInstanceChannels` (full reload, 1h) and `loopErrorInstanceChannels`
/// (down-instance recheck, 1m). The caller does not await these — the server
/// starts accepting connections immediately while channels load in the
/// background, same as the original.
pub fn spawn_loops(state: Arc<AppState>) {
    tokio::spawn({
        let state = state.clone();
        async move {
            load_instance_channels(&state, false).await;
            let mut interval =
                tokio::time::interval(Duration::from_millis(RELOAD_INTERVAL_MS as u64));
            interval.tick().await; // first tick fires immediately; skip it, we just loaded
            loop {
                interval.tick().await;
                load_instance_channels(&state, false).await;
            }
        }
    });

    tokio::spawn(async move {
        load_instance_channels(&state, true).await;
        let mut interval = tokio::time::interval(Duration::from_millis(ERROR_INTERVAL_MS as u64));
        interval.tick().await;
        loop {
            interval.tick().await;
            load_instance_channels(&state, true).await;
        }
    });
}
