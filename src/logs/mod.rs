pub mod instance;
pub mod mirror;
pub mod namehistory;
pub mod recent_messages;

use serde::{Deserialize, Serialize};

/// A channel entry as returned by a justlog/rustlog instance's `/channels`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Channel {
    pub name: String,
    #[serde(rename = "userID")]
    pub user_id: String,
}

/// One entry of an instance's `/list` `availableLogs` array: a day (channel
/// logs) or a month (user logs).
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct AvailableLogDate {
    pub year: String,
    pub month: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub day: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Elapsed {
    pub ms: f64,
    pub s: f64,
}

impl Elapsed {
    pub fn since(start: std::time::Instant) -> Elapsed {
        let ms = start.elapsed().as_secs_f64() * 1000.0;
        Elapsed {
            ms: (ms * 100.0).round() / 100.0,
            s: (ms / 10.0).round() / 100.0,
        }
    }
}
