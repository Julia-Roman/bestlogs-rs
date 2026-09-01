use serde::{Deserialize, Serialize};

use crate::{http_client, state::AppState, util};

#[derive(Debug, Clone, Deserialize)]
struct IvrUser {
    id: String,
    login: String,
    #[serde(rename = "displayName")]
    display_name: String,
    logo: String,
    #[serde(default)]
    banned: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct TwitchUser {
    pub name: String,
    pub login: String,
    pub avatar: String,
    pub id: String,
    pub banned: bool,
}

/// Port of `Utils.getInfo`: resolves a login or `id:`-prefixed id via
/// api.ivr.fi, cached by the same key shape as the original.
///
/// Uses `try_get_with` so that if several requests ask about the same
/// channel/user at once (very common — a burst of viewers hitting `/rdr`
/// for a channel that just went live), only one of them actually calls out
/// to ivr.fi; the rest wait on that single in-flight lookup instead of each
/// firing their own redundant request. A failed lookup is never cached, so
/// a transient ivr.fi hiccup doesn't poison the result for other callers.
pub async fn get_info(state: &AppState, user: &str) -> anyhow::Result<TwitchUser> {
    let cache_key = user.replacen("id:", "id-", 1);

    state
        .caches
        .info_data
        .try_get_with(cache_key, fetch_info(state, user))
        .await
        .map_err(|err| anyhow::anyhow!("{err}"))
}

async fn fetch_info(state: &AppState, user: &str) -> anyhow::Result<TwitchUser> {
    let is_id = util::USER_ID_REGEX.is_match(user);
    let clean = util::strip_id_prefix(user);
    let query = if is_id {
        ("id", clean)
    } else {
        ("login", clean)
    };

    let response = state
        .http
        .get("https://api.ivr.fi/v2/twitch/user")
        .query(&[query])
        .timeout(http_client::LIST_TIMEOUT)
        .send()
        .await?;

    if !response.status().is_success() {
        anyhow::bail!("ivr.fi responded with {}", response.status());
    }

    let body: Vec<IvrUser> = response.json().await?;
    let Some(data) = body.into_iter().next() else {
        anyhow::bail!("user not found: {user}");
    };

    let name = if data.display_name.to_lowercase() == data.login {
        data.display_name
    } else {
        data.login.clone()
    };

    Ok(TwitchUser {
        name,
        login: data.login,
        avatar: data.logo,
        id: data.id,
        banned: data.banned,
    })
}
