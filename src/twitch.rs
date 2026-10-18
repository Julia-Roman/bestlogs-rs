use serde::de::IgnoredAny;
use serde::{Deserialize, Serialize};

use crate::{http_client, state::AppState, util};

const GQL_URL: &str = "https://gql.twitch.tv/gql";
const GQL_CLIENT_ID: &str = "ue6666qo983tsx6so1t0vnawi233wa";

/// `all` finds suspended and deactivated accounts too, `active` only ones in
/// good standing — a user only the first one finds is banned.
const USER_QUERY: &str = "query($login: String, $id: ID) { \
    all: user(login: $login, id: $id, lookupType: ALL) { id login displayName profileImageURL(width: 600) } \
    active: user(login: $login, id: $id, lookupType: ACTIVE) { id } }";

#[derive(Deserialize)]
struct GqlResponse {
    data: Option<GqlData>,
}

#[derive(Deserialize)]
struct GqlData {
    all: Option<GqlUser>,
    active: Option<IgnoredAny>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GqlUser {
    id: String,
    login: String,
    display_name: String,
    #[serde(rename = "profileImageURL")]
    profile_image_url: String,
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
/// Twitch's GQL API, cached by the same key shape as the original.
///
/// Uses `try_get_with` so that if several requests ask about the same
/// channel/user at once (very common — a burst of viewers hitting `/rdr`
/// for a channel that just went live), only one of them actually calls out
/// to Twitch; the rest wait on that single in-flight lookup instead of each
/// firing their own redundant request. A failed lookup is never cached, so
/// a transient GQL hiccup doesn't poison the result for other callers.
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
    let clean = util::strip_id_prefix(user);
    let variables = if util::USER_ID_REGEX.is_match(user) {
        serde_json::json!({ "id": clean })
    } else {
        serde_json::json!({ "login": clean })
    };

    let response = state
        .http
        .post(GQL_URL)
        .header("Client-Id", GQL_CLIENT_ID)
        .json(&serde_json::json!({
            "query": USER_QUERY,
            "variables": variables,
        }))
        .timeout(http_client::LIST_TIMEOUT)
        .send()
        .await?;

    if !response.status().is_success() {
        anyhow::bail!("Twitch GQL responded with {}", response.status());
    }

    // `data` is null when the query itself failed (GQL reports that in an
    // `errors` array with a 200), which is not the same as "no such user".
    let Some(body) = response.json::<GqlResponse>().await?.data else {
        anyhow::bail!("Twitch GQL returned no data for {user}");
    };
    let Some(data) = body.all else {
        anyhow::bail!("user not found: {user}");
    };
    let banned = body.active.is_none();

    let name = if data.display_name.to_lowercase() == data.login {
        data.display_name
    } else {
        data.login.clone()
    };

    Ok(TwitchUser {
        name,
        login: data.login,
        avatar: data.profile_image_url,
        id: data.id,
        banned,
    })
}
