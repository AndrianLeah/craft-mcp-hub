//! The hub's settings: which apps are set up, on which port, and how custom apps are started.

use crate::paths;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub apps: BTreeMap<String, AppConfig>,
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
    /// That folder (default: the home folder).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub custom: bool,
}

impl Config {
    /// The saved settings; empty when there are none yet or the file can't be read.
    pub fn load() -> Config {
        fs::read_to_string(paths::config_file()).ok().and_then(|text| toml::from_str(&text).ok()).unwrap_or_default()
    }

    pub fn save(&self) -> Result<(), String> {
        let file = paths::config_file();
        let text = toml::to_string_pretty(self).map_err(|e| e.to_string())?;
        fs::create_dir_all(paths::hub_home()).map_err(|e| format!("cannot create {}: {e}", paths::hub_home().display()))?;
        // Written beside the file and moved over it, so a reader never sees half a file.
        let temp = file.with_extension("toml.tmp");
        fs::write(&temp, text).and_then(|_| fs::rename(&temp, &file)).map_err(|e| format!("cannot write {}: {e}", file.display()))
    }
}
