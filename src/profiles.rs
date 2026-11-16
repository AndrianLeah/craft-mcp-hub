//! How an app's command-line tool is started as a windowless MCP engine.

use crate::proc;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::OnceLock;

const BUILTIN: &str = include_str!("../assets/profiles.toml");

/// First port given to an app that has no default of its own.
pub const FIRST_FREE_PORT: u16 = 7830;

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Profile {
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default = "default_args")]
    pub args: Vec<String>,
    #[serde(default)]
    pub library: bool,
    #[serde(default)]
    pub settings: Vec<String>,
    #[serde(default)]
    pub root_flags: Vec<String>,
    /// The arguments are not known: work them out from the tool's help when the app is added.
    #[serde(default)]
    pub probe: bool,
}

fn default_args() -> Vec<String> {
    vec!["mcp".to_string()]
}

impl Default for Profile {
    fn default() -> Profile {
        Profile { port: None, args: default_args(), library: false, settings: Vec::new(), root_flags: Vec::new(), probe: true }
    }
}

fn all() -> &'static BTreeMap<String, Profile> {
    static ALL: OnceLock<BTreeMap<String, Profile>> = OnceLock::new();
    ALL.get_or_init(|| toml::from_str(BUILTIN).expect("assets/profiles.toml is valid"))
}

pub fn builtin(id: &str) -> Option<&'static Profile> {
    all().get(id)
}

/// The default ports of every known app, which a new app must not be given.
pub fn reserved_ports() -> Vec<u16> {
    all().values().filter_map(|p| p.port).collect()
}

/// Works out how to start a tool we have no profile for, from what its help says.
pub fn probe(cli: &[String]) -> Profile {
    let Some((program, prefix)) = cli.split_first() else { return Profile::default() };
    let help = |extra: &[&str]| {
        let args: Vec<String> = prefix.iter().cloned().chain(extra.iter().map(|s| s.to_string())).collect();
        proc::run(Path::new(program), &args).text
    };
    from_help(&format!("{}\n{}", help(&["--help"]), help(&["mcp", "--help"])))
}

fn from_help(help: &str) -> Profile {
    let mut profile = Profile { probe: false, ..Profile::default() };
    // Some tools look for the desktop app first unless told to run on their own.
    if help.contains("--headless") {
        profile.args.push("--headless".to_string());
    }
    profile.library = help.contains("--library DIR") || help.contains("--library <");
    if profile.library && help.contains("--compact") {
        profile.args.push("--compact".to_string());
    }
    if help.contains("an omitted root denies") {
        profile.root_flags = vec!["--automation-read-root".to_string(), "--automation-write-root".to_string()];
    }
    profile
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_profiles_parse() {
        assert_eq!(builtin("vectorcraft").unwrap().args, ["mcp", "--headless"]);
        assert!(builtin("lightcraft").unwrap().library);
        assert_eq!(builtin("pdfcraft").unwrap().args, ["mcp"]);
        assert!(!builtin("pdfcraft").unwrap().probe);
        assert!(builtin("filmcraft").unwrap().probe);
        let mut ports = reserved_ports();
        ports.sort_unstable();
        ports.dedup();
        assert_eq!(ports.len(), reserved_ports().len(), "default ports are unique");
    }

    #[test]
    fn probes_from_help_text() {
        let library = from_help("mcp [OPTIONS]\n  --compact  list only the helper tools\n  --library DIR  open a library");
        assert_eq!(library.args, ["mcp", "--compact"]);
        assert!(library.library);
        let vector = from_help("x-cli mcp [--connect ADDR | --headless]");
        assert_eq!(vector.args, ["mcp", "--headless"]);
        assert!(!vector.library);
        let photo = from_help("Headless: each automation root grants that direction of file access; an omitted root denies it.");
        assert_eq!(photo.root_flags.len(), 2);
    }
}
