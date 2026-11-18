//! The hub's settings: which apps are set up, on which port, and how custom apps are started.

use crate::{files, paths};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::time::Duration;

/// How many minutes an engine may sit with no session before its server stops it, unless the
/// settings say otherwise.
pub const IDLE_STOP_MINUTES: f64 = 10.0;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    /// The AI clients apps are registered with, by id. Not set: Claude Code, as before clients
    /// could be chosen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clients: Option<Vec<String>>,
    /// Minutes an engine may sit with no session before its server stops it to free its memory (it
    /// starts again on the next request), for every app that doesn't say otherwise; a fraction
    /// (`0.5`) is fine. Not set: [`IDLE_STOP_MINUTES`]. 0: engines keep running.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idle_stop_minutes: Option<f64>,
    /// AI clients the hub doesn't know by itself, as the user (or their AI) described them.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub custom_clients: BTreeMap<String, CustomClient>,
    #[serde(default)]
    pub apps: BTreeMap<String, AppConfig>,
}

/// Where an AI client keeps its MCP servers, for a client the hub has no built-in knowledge of.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CustomClient {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// The settings file.
    pub file: String,
    /// `json` (the default) or `toml`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub format: String,
    /// The object (JSON) or table (TOML) holding the servers. Default: `mcpServers`, `mcp_servers`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub key: String,
    /// JSON only: whether each server entry carries `"type": "stdio"`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub typed: bool,
    /// Whether the client reads its servers only when it starts.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub restart: bool,
}

/// One app that has been set up. Everything but the port is optional: an app from the catalog is
/// found on disk and started the way its built-in profile says.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AppConfig {
    pub port: u16,
    /// Display name, for an app the catalog doesn't know.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// How to run the app's command-line tool, for a custom app (or to override the one found).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cli: Vec<String>,
    /// Arguments that start the tool as an MCP engine, instead of the built-in profile's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
    /// Whether the app works on photo libraries the hub opens for it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub library: Option<bool>,
    /// The app's settings folders, where its recent libraries are listed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<Vec<String>>,
    /// Flags that each take the folder the engine may read and write.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_flags: Option<Vec<String>>,
    /// Whether those flags may be repeated, for several folders.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub multi_root: Option<bool>,
    /// Whether the engine has no file access without a folder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_required: Option<bool>,
    /// The folders the engine may read and write. Not set: the home folder. Empty: any file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub roots: Option<Vec<String>>,
    /// One folder, as it was saved before an engine could have several. Read, never written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub custom: bool,
    /// Minutes this app's engine may sit with no session before it is stopped, instead of the
    /// hub-wide setting. 0: it keeps running.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idle_stop_minutes: Option<f64>,
}

impl Config {
    /// The saved settings; empty when there are none yet or the file can't be read.
    pub fn load() -> Config {
        fs::read_to_string(paths::config_file()).ok().and_then(|text| toml::from_str(&text).ok()).unwrap_or_default()
    }

    pub fn save(&self) -> Result<(), String> {
        let file = paths::config_file();
        let text = toml::to_string_pretty(self).map_err(|e| e.to_string())?;
        files::write_atomic(&file, text.as_bytes()).map_err(|e| format!("cannot write {}: {e}", file.display()))
    }

    /// How long the app's engine may sit with no session before its server stops it: the app's own
    /// setting, else the hub-wide one, else [`IDLE_STOP_MINUTES`]. `None` when it keeps running.
    pub fn idle_stop(&self, app: &str) -> Option<Duration> {
        let own = self.apps.get(app).and_then(|saved| saved.idle_stop_minutes);
        let minutes = own.or(self.idle_stop_minutes).unwrap_or(IDLE_STOP_MINUTES);
        // 0 (or anything that isn't a positive number) keeps it running, and so does a wait too
        // long to count.
        if minutes.is_nan() || minutes <= 0.0 {
            return None;
        }
        Duration::try_from_secs_f64(minutes * 60.0).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_stop_is_the_apps_own_then_the_hubs_then_ten_minutes() {
        let mut config: Config = toml::from_str("[apps.photocraft]\nport = 7801\n").unwrap();
        assert_eq!(config.idle_stop("photocraft"), Some(Duration::from_secs(10 * 60)));
        assert_eq!(config.idle_stop("unknown"), Some(Duration::from_secs(10 * 60)));

        config = toml::from_str("idle_stop_minutes = 30\n[apps.photocraft]\nport = 7801\n[apps.figcraft]\nport = 7802\nidle_stop_minutes = 2\n").unwrap();
        assert_eq!(config.idle_stop("photocraft"), Some(Duration::from_secs(30 * 60)));
        assert_eq!(config.idle_stop("figcraft"), Some(Duration::from_secs(2 * 60)));

        // 0 keeps engines running: everywhere, or for one app.
        config.idle_stop_minutes = Some(0.0);
        assert_eq!(config.idle_stop("photocraft"), None);
        assert_eq!(config.idle_stop("figcraft"), Some(Duration::from_secs(2 * 60)));
        config.apps.get_mut("figcraft").unwrap().idle_stop_minutes = Some(0.0);
        config.idle_stop_minutes = None;
        assert_eq!(config.idle_stop("figcraft"), None);
        assert_eq!(config.idle_stop("photocraft"), Some(Duration::from_secs(10 * 60)));
    }

    #[test]
    fn idle_stop_takes_fractions_and_ignores_nonsense() {
        let config: Config = toml::from_str("idle_stop_minutes = 0.5\n").unwrap();
        assert_eq!(config.idle_stop("any"), Some(Duration::from_secs(30)));
        for nonsense in [-1.0, f64::NAN, f64::INFINITY, 1e300] {
            let config = Config { idle_stop_minutes: Some(nonsense), ..Config::default() };
            assert_eq!(config.idle_stop("any"), None, "{nonsense}");
        }
    }

    #[test]
    fn idle_stop_survives_saving() {
        let mut config = Config { clients: Some(vec!["claude-code".into()]), idle_stop_minutes: Some(25.0), ..Config::default() };
        config.apps.insert("photocraft".into(), AppConfig { port: 7801, idle_stop_minutes: Some(0.0), ..AppConfig::default() });
        let text = toml::to_string_pretty(&config).unwrap();
        let back: Config = toml::from_str(&text).unwrap();
        assert_eq!(back.idle_stop_minutes, Some(25.0));
        assert_eq!(back.apps["photocraft"].idle_stop_minutes, Some(0.0));
        // Not set: not written, so a file saved before the setting existed stays as it was.
        let plain = toml::to_string_pretty(&Config::default()).unwrap();
        assert!(!plain.contains("idle_stop"));
    }
}
