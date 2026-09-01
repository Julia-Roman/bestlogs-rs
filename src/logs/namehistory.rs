use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::http_client;
use crate::state::AppState;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PreviousName {
    pub user_login: String,
    pub last_timestamp: DateTime<Utc>,
    pub first_timestamp: DateTime<Utc>,
}

pub struct NameHistoryResult {
    pub source_instances: Vec<String>,
    pub name_history: Vec<PreviousName>,
}

/// Port of `Utils.getNameHistory`: resolves a `login:`-prefixed username to
/// an id, then merges name-history entries reported by every alive instance.
pub async fn get_name_history(
    state: &Arc<AppState>,
    user: &str,
) -> Result<NameHistoryResult, String> {
    let stripped = user.strip_prefix("id:").unwrap_or(user);

    let user_id = if let Some(login) = stripped.strip_prefix("login:") {
        match crate::twitch::get_info(state, login).await {
            Ok(info) => info.id,
            Err(_) => {
                return Ok(NameHistoryResult {
                    source_instances: Vec::new(),
                    name_history: Vec::new(),
                });
            }
        }
    } else if stripped.chars().all(|c| c.is_ascii_digit()) && !stripped.is_empty() {
        stripped.to_string()
    } else {
        return Err(
            "The value must be an ID or use 'login:' to refer to usernames. Example: 754201843 or login:zonianmidian"
                .to_string(),
        );
    };

    let instances = state.alive_instances();
    let results: Vec<(String, Option<Vec<PreviousName>>)> =
        futures::future::join_all(instances.iter().map(|key| {
            let user_id = user_id.clone();
            async move {
                let host = state.instance_host(key);
                let response = state
                    .http
                    .get(format!("https://{host}/namehistory/{user_id}"))
                    .timeout(http_client::RELOAD_TIMEOUT)
                    .send()
                    .await;

                let entries = match response {
                    Ok(res) if res.status().is_success() => {
                        res.json::<Vec<PreviousName>>().await.ok()
                    }
                    _ => None,
                };

                (host, entries)
            }
        }))
        .await;

    let mut source_instances = Vec::new();
    let mut name_history: Vec<PreviousName> = Vec::new();

    for (host, entries) in results {
        let Some(entries) = entries else { continue };
        info!(
            "[{host}] Found {} registered usernames for ID {user_id}",
            entries.len()
        );

        if !entries.is_empty() {
            source_instances.push(format!("https://{host}"));
        }

        for entry in entries {
            if let Some(existing) = name_history
                .iter_mut()
                .find(|e| e.user_login == entry.user_login)
            {
                if entry.last_timestamp > existing.last_timestamp {
                    existing.last_timestamp = entry.last_timestamp;
                }
                if entry.first_timestamp < existing.first_timestamp {
                    existing.first_timestamp = entry.first_timestamp;
                }
            } else {
                name_history.push(entry);
            }
        }
    }

    name_history.sort_by_key(|a| a.last_timestamp);

    info!(
        "[NameHistory] Found {} unique usernames for ID {user_id}",
        name_history.len()
    );

    Ok(NameHistoryResult {
        source_instances,
        name_history,
    })
}
