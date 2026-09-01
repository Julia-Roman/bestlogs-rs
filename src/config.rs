use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

/// Baked into the binary so a working default config always exists, even if
/// `config.json` is missing next to the executable at runtime.
const DEFAULT_CONFIG_JSON: &str = include_str!("../example_config.json");

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct InstanceMeta {
    pub maintainer: Option<String>,
    pub message: Option<String>,
    pub country: Option<String>,
    pub city: Option<String>,
    pub flag: Option<String>,
    pub url: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct JustlogInstanceMeta {
    pub maintainer: Option<String>,
    /// Optional alternate hostname to actually connect to, while the outer
    /// map key stays the public/display hostname (matches the original's
    /// `justlogsInstances[url].alternate` override).
    pub alternate: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct RecentMessagesInstanceMeta {
    pub maintainer: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct UmamiConfig {
    #[serde(default)]
    pub token: String,
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub url: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    pub port: u16,
    #[serde(default)]
    pub instance: InstanceMeta,
    #[serde(default)]
    pub justlogs_instances: IndexMap<String, JustlogInstanceMeta>,
    #[serde(default)]
    pub recentmessages_instances: IndexMap<String, RecentMessagesInstanceMeta>,
    pub umami_stats: Option<UmamiConfig>,
}

impl Config {
    /// Mirrors the original `loadConfig()`: `example_config.json` (compiled
    /// in) supplies defaults, and a `config.json` in the working directory,
    /// if present, overrides them key-by-key at the top level.
    ///
    /// Never fails: a missing `config.json` is expected (defaults only), and
    /// a *broken* one (invalid JSON, or fields that don't match the schema)
    /// is logged as a warning and ignored in favor of the built-in defaults
    /// — a config typo should degrade the deployment, not take the whole
    /// service down.
    pub fn load() -> Config {
        let defaults: serde_json::Value = serde_json::from_str(DEFAULT_CONFIG_JSON)
            .expect("built-in example_config.json must be valid JSON");

        let merged = match std::fs::read_to_string("config.json") {
            Ok(contents) => match serde_json::from_str::<serde_json::Value>(&contents) {
                Ok(custom) => {
                    let mut merged = defaults.clone();
                    if let (Some(merged_obj), Some(custom_obj)) =
                        (merged.as_object_mut(), custom.as_object())
                    {
                        for (key, value) in custom_obj {
                            merged_obj.insert(key.clone(), value.clone());
                        }
                    }
                    merged
                }
                Err(err) => {
                    tracing::warn!(
                        "config.json is not valid JSON ({err}); falling back to built-in defaults"
                    );
                    defaults.clone()
                }
            },
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => defaults.clone(),
            Err(err) => {
                tracing::warn!(
                    "Failed to read config.json ({err}); falling back to built-in defaults"
                );
                defaults.clone()
            }
        };

        match serde_json::from_value(merged) {
            Ok(config) => config,
            Err(err) => {
                tracing::warn!(
                    "config.json has invalid fields ({err}); falling back to built-in defaults"
                );
                serde_json::from_value(defaults)
                    .expect("built-in example_config.json must match Config's schema")
            }
        }
    }
}
