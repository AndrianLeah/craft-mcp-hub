//! The apps as the hub sees them, and what can be done with them: add, turn on and off, move to
//! another port, check.

use crate::catalog::{self, Catalog, CatalogApp};
use crate::clients::{self, Snapshot};
use crate::config::{AppConfig, Config};
use crate::control::{self, CheckOk, Probe, Status};
use crate::profiles::{self, Profile};
use crate::{discover, paths, proc, VERSION};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Official,
    Community,
    /// Added by the user: a fork or a build the catalog doesn't know.
    Custom,
}

#[derive(Debug, Clone)]
pub struct App {
    pub id: String,
    pub name: String,
    pub tagline: String,
    pub origin: Origin,
    pub color: [u8; 3],
    /// How to run the app's command-line tool; `None` when the app isn't installed.
    pub cli: Option<Vec<String>>,
    pub profile: Profile,
    /// The port of the app's server: the configured one, else the default it would get.
    pub port: u16,
    /// Whether the app has been added to the hub.
    pub configured: bool,
    root: Option<String>,
}

impl App {
    pub fn installed(&self) -> bool {
        self.cli.is_some()
    }

    /// The folder the engine may read and write, for an app that needs to be given one.
    pub fn root(&self) -> Option<String> {
        (!self.profile.root_flags.is_empty()).then(|| self.root.clone().unwrap_or_else(|| paths::home_dir().to_string_lossy().into_owned()))
    }

    /// The full command line of the app's windowless MCP engine.
    pub fn engine_command(&self) -> Option<Vec<String>> {
        let mut command = self.cli.clone()?;
        command.extend(self.profile.args.iter().cloned());
        if let Some(root) = self.root() {
            for flag in &self.profile.root_flags {
                command.push(flag.clone());
                command.push(root.clone());
            }
        }
        Some(command)
    }

    /// The app's settings files that list its recent libraries.
    pub fn recent_files(&self) -> Vec<PathBuf> {
        let base = paths::app_config_dir();
        self.profile.settings.iter().flat_map(|folder| ["ui.json", "recent-catalogs.json"].map(|file| base.join(folder).join(file))).collect()
    }
}

/// A stable colour for an app the catalog gives none.
fn color_for(id: &str) -> [u8; 3] {
    const PALETTE: [[u8; 3]; 6] = [[195, 139, 255], [91, 209, 230], [245, 185, 53], [61, 214, 140], [255, 107, 74], [140, 155, 255]];
    PALETTE[id.bytes().fold(0usize, |sum, b| sum.wrapping_mul(31).wrapping_add(b as usize)) % PALETTE.len()]
}

pub struct Hub {
    pub config: Config,
    pub catalog: Catalog,
}

impl Hub {
    pub fn load() -> Hub {
        Hub { config: Config::load(), catalog: catalog::load() }
    }

    fn resolve(&self, id: &str) -> Option<App> {
        let listed = self.catalog.get(id);
        let saved = self.config.apps.get(id);
        if listed.is_none() && saved.is_none() {
            return None;
        }
        let mut profile = profiles::builtin(id).cloned().unwrap_or_default();
        if let Some(saved) = saved {
            if let Some(args) = &saved.args {
                profile.args = args.clone();
                profile.probe = false;
            }
            profile.library = saved.library.unwrap_or(profile.library);
            profile.settings = saved.settings.clone().unwrap_or(profile.settings);
            profile.root_flags = saved.root_flags.clone().unwrap_or(profile.root_flags);
        }
        let given = saved.map(|s| &s.cli).filter(|cli| !cli.is_empty());
        let cli = match given {
            Some(cli) => cli_exists(cli).then(|| cli.clone()),
            None => listed.and_then(discover::find_cli),
        };
        let custom = listed.is_none() || saved.is_some_and(|s| s.custom);
        Some(App {
            id: id.to_string(),
            name: saved.and_then(|s| s.name.clone()).or_else(|| listed.map(|l| l.name.clone())).filter(|n| !n.is_empty()).unwrap_or_else(|| id.to_string()),
            tagline: listed.map(|l| l.tagline.clone()).unwrap_or_default(),
            origin: match listed {
                _ if custom => Origin::Custom,
                Some(CatalogApp { kind, .. }) if kind == "community" => Origin::Community,
                _ => Origin::Official,
            },
            color: listed.and_then(|l| l.color).unwrap_or_else(|| color_for(id)),
            cli,
            port: saved.map(|s| s.port).or(profile.port).unwrap_or(0),
            profile,
            configured: saved.is_some(),
            root: saved.and_then(|s| s.root.clone()),
        })
    }

    pub fn app(&self, id: &str) -> Option<App> {
        self.resolve(id)
    }

    /// Every app: the catalog's in its order, then the custom ones.
    pub fn apps(&self) -> Vec<App> {
        let listed = self.catalog.apps.iter().map(|a| a.id.as_str());
        let custom = self.config.apps.keys().map(String::as_str).filter(|id| self.catalog.get(id).is_none());
        listed.chain(custom).filter_map(|id| self.resolve(id)).collect()
    }
}

fn cli_exists(cli: &[String]) -> bool {
    cli.first().is_some_and(|program| Path::new(program).is_file() || paths::find_in_path(program).is_some())
}

// ---------------------------------------------------------------- state

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateKind {
    NotInstalled,
    /// Installed, not added to the hub yet.
    New,
    Off,
    /// Registered in the client by something other than this hub.
    Other,
    /// Registered; the server starts with the next client session.
    Waiting,
    Starting,
    On,
    Error,
    PortBusy,
}

#[derive(Debug, Clone)]
pub struct AppState {
    pub kind: StateKind,
    pub label: String,
    pub detail: String,
    /// The long explanation, for a tooltip or the diagnosis.
    pub tip: String,
    pub library: Option<String>,
    pub status: Option<Status>,
    /// The running server was started with other settings or by another version.
    pub stale: bool,
    /// The app is on, but some enabled client doesn't have it: says which.
    pub partial: Option<String>,
}

impl AppState {
    fn new(kind: StateKind, label: &str, detail: impl Into<String>, tip: impl Into<String>) -> AppState {
        AppState { kind, label: label.to_string(), detail: detail.into(), tip: tip.into(), library: None, status: None, stale: false, partial: None }
    }

    /// Whether the app is registered with a client by this hub.
    pub fn is_on(&self) -> bool {
        matches!(self.kind, StateKind::Waiting | StateKind::Starting | StateKind::On | StateKind::Error | StateKind::PortBusy)
    }
}

pub fn short_error(error: &str) -> &'static str {
    if error.contains("already open") {
        "library open elsewhere"
    } else if error.contains("Could not start the engine") {
        "program not found"
    } else {
        "see Diagnose"
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    match n {
        0 => format!("no {many}"),
        1 => format!("1 {one}"),
        _ => format!("{n} {many}"),
    }
}

/// What an app is doing: its registration in the enabled clients plus what really answers on its port.
pub fn state(app: &App, registered: &Snapshot) -> AppState {
    if !app.installed() {
        let detail = if app.configured { "its command-line tool is gone" } else { "" };
        return AppState::new(StateKind::NotInstalled, "Not installed", detail, "");
    }
    let summary = registered.of(&app.id);
    let probe = if app.port == 0 { Probe::Down } else { control::status(app.port) };
    if summary.ours.is_empty() {
        return match summary.other.first() {
            Some((client, what)) => AppState::new(
                StateKind::Other,
                "Other setup",
                format!("registered by {what}"),
                format!(
                    "{} has this name registered by something other than this hub. Turning it on here replaces that (a copy is kept in the hub's backup folder).",
                    client.name
                ),
            ),
            None if !app.configured => AppState::new(StateKind::New, "New", "installed, not added yet", "Add it to give it a port and turn it on."),
            None => {
                let running = matches!(&probe, Probe::Up(s) if s.app == app.id);
                AppState::new(StateKind::Off, "Off", if running { "server still running" } else { "" }, "")
            }
        };
    }
    let absent: Vec<&str> = summary.missing.iter().map(|client| client.name).chain(summary.other.iter().map(|(client, _)| client.name)).collect();
    let mut state = running_state(app, probe);
    if !absent.is_empty() {
        state.partial = Some(format!("not in {}", absent.join(", ")));
    }
    state
}

/// The state of an app that is registered: what its server is doing.
fn running_state(app: &App, probe: Probe) -> AppState {
    match probe {
        Probe::Down => AppState::new(StateKind::Waiting, "Waiting", "starts with the next session", "No server is running: the next client session starts it."),
        Probe::Foreign(who) => {
            let tip = format!("Port {} is used by {who}. Choose another port.", app.port);
            AppState::new(StateKind::PortBusy, "Port in use", who, tip)
        }
        Probe::Up(status) if status.app != app.id => {
            let tip = format!("Port {} is used by the hub server of {}. Choose another port.", app.port, status.app);
            AppState::new(StateKind::PortBusy, "Port in use", format!("by {}", status.app), tip)
        }
        Probe::Up(status) => {
            let expected = proc::display_command(&app.engine_command().unwrap_or_default());
            let stale = status.version != VERSION || status.engine.command != expected;
            let mut state = match status.engine.state.as_str() {
                "ready" => {
                    let tip = format!("Server pid {} · engine {} (pid {})", status.pid, status.engine.server, status.engine.pid);
                    AppState::new(StateKind::On, "On", plural(status.sessions, "session", "sessions"), tip)
                }
                "failed" => AppState::new(StateKind::Error, "Error", short_error(&status.last_error), status.last_error.clone()),
                _ => AppState::new(StateKind::Starting, "Starting", "engine starting", ""),
            };
            if stale {
                state.tip = format!("{} Restart it to apply the current settings.", state.tip).trim().to_string();
            }
            state.stale = stale;
            state.library = status.library.clone();
            state.status = Some(*status);
            state
        }
    }
}

// ---------------------------------------------------------------- actions

fn unknown(id: &str) -> String {
    format!("`{id}` is not an app the hub knows. `crafthub list` shows them; a custom build is added with `crafthub add {id} --cli <path>`.")
}

/// Why this app can't use the port, or `None` when it can.
pub fn port_conflict(config: &Config, id: &str, port: u16) -> Option<String> {
    if port < 1024 {
        return Some(format!("Port {port} is reserved by the system: use 1024 or higher."));
    }
    if let Some((other, _)) = config.apps.iter().find(|(other, app)| other.as_str() != id && app.port == port) {
        return Some(format!("Port {port} is already given to {other}."));
    }
    match control::status(port) {
        Probe::Down => None,
        Probe::Up(status) if status.app == id => None,
        Probe::Up(status) => Some(format!("Port {port} is already used by the hub server of {}.", status.app)),
        Probe::Foreign(who) => Some(format!("Port {port} is already used by {who}.")),
    }
}

fn free_port(config: &Config, preferred: Option<u16>) -> u16 {
    let taken = |port: u16| config.apps.values().any(|a| a.port == port);
    if let Some(port) = preferred.filter(|p| !taken(*p)) {
        return port;
    }
    let reserved = profiles::reserved_ports();
    (profiles::FIRST_FREE_PORT..u16::MAX).find(|p| !taken(*p) && !reserved.contains(p) && !control::port_in_use(*p)).unwrap_or(profiles::FIRST_FREE_PORT)
}

#[derive(Debug, Default, Clone)]
pub struct AddOptions {
    /// The command-line tool of a custom app (or of a catalog app installed somewhere unusual).
    pub cli: Option<String>,
    pub name: Option<String>,
    /// Start it like this known app instead of working it out from its help.
    pub like: Option<String>,
    pub port: Option<u16>,
    /// The app's settings folders, where its recent libraries are listed.
    pub settings: Vec<String>,
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 40 && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-') && !id.starts_with('-')
}

/// Adds an app to the hub: gives it a port and works out how to start it. It is not turned on.
pub fn add(id: &str, options: AddOptions) -> Result<App, String> {
    if !valid_id(id) {
        return Err(format!("`{id}` can't be an app id: use lowercase letters, digits and dashes."));
    }
    let mut hub = Hub::load();
    let listed = hub.catalog.get(id).cloned();
    let mut saved = hub.config.apps.get(id).cloned().unwrap_or_default();
    let mut profile = profiles::builtin(id).cloned().unwrap_or_default();

    let cli = match &options.cli {
        Some(path) => {
            if !Path::new(path).is_file() {
                return Err(format!("There is no file at {path}."));
            }
            saved.cli = vec![path.clone()];
            // A catalog app installed somewhere unusual keeps its own profile.
            saved.custom = listed.is_none();
            saved.cli.clone()
        }
        None if !saved.cli.is_empty() => saved.cli.clone(),
        None => match &listed {
            Some(listed) => discover::find_cli(listed).ok_or_else(|| {
                format!("{} is not installed (its command-line tool was not found). If it is somewhere unusual, pass --cli <path>.", listed.name)
            })?,
            None => return Err(unknown(id)),
        },
    };
    if let Some(name) = options.name {
        saved.name = Some(name);
    }

    // A custom app, or one whose profile isn't verified, is started the way a known app is
    // (--like) or the way its own help suggests.
    if let Some(like) = &options.like {
        let model = profiles::builtin(like).filter(|p| !p.probe).ok_or_else(|| format!("`{like}` has no verified profile to copy."))?;
        profile = model.clone();
        profile.settings.clear();
    } else if saved.custom || profile.probe {
        let default_port = profile.port;
        profile = profiles::probe(&cli);
        profile.port = default_port;
    }
    if saved.custom || options.like.is_some() || profiles::builtin(id).is_none_or(|p| p.probe) {
        saved.args = Some(profile.args.clone());
        saved.library = Some(profile.library);
        saved.root_flags = Some(profile.root_flags.clone());
        if profile.library {
            let name = saved.name.clone().or_else(|| listed.as_ref().map(|l| l.name.clone())).unwrap_or_else(|| id.to_string());
            let folders = if options.settings.is_empty() { saved.settings.clone().unwrap_or_else(|| vec![name]) } else { options.settings.clone() };
            saved.settings = Some(folders);
        }
    }

    let wanted = options.port.or((saved.port != 0).then_some(saved.port));
    saved.port = match wanted {
        Some(port) => {
            if let Some(conflict) = port_conflict(&hub.config, id, port) {
                return Err(conflict);
            }
            port
        }
        None => free_port(&hub.config, if saved.custom { None } else { profile.port }),
    };
    hub.config.apps.insert(id.to_string(), saved);
    hub.config.save()?;
    Hub::load().app(id).ok_or_else(|| unknown(id))
}

/// Starts the app's server (restarting one that runs with other settings) and waits for its engine.
pub fn start_server(app: &App) -> Result<(), String> {
    let expected = proc::display_command(&app.engine_command().ok_or_else(|| format!("{} is not installed.", app.name))?);
    if let Probe::Up(running) = control::status(app.port) {
        if running.app == app.id && (running.version != VERSION || running.engine.command != expected) {
            control::shutdown(app.port, &app.id);
        }
    }
    let mut status = control::ensure_server(&app.id, app.port, Duration::from_secs(25))?;
    let deadline = Instant::now() + Duration::from_secs(30);
    while status.engine.state == "starting" && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(250));
        match control::status(app.port) {
            Probe::Up(now) => status = *now,
            _ => break,
        }
    }
    if status.engine.state == "failed" {
        return Err(format!("The server is up, but the {} engine does not start: {}", app.name, status.last_error));
    }
    Ok(())
}

/// Turns an app on: adds it if needed, registers it with the client, starts its server.
pub fn turn_on(id: &str) -> Result<(), String> {
    let mut app = Hub::load().app(id).ok_or_else(|| unknown(id))?;
    if !app.installed() {
        return Err(format!("{} is not installed.", app.name));
    }
    if !app.configured {
        app = add(id, AddOptions::default())?;
    }
    let config = Config::load();
    if let Some(conflict) = port_conflict(&config, id, app.port) {
        return Err(conflict);
    }
    let enabled = clients::enabled(&config);
    if enabled.is_empty() {
        return Err(no_client_hint());
    }
    let failures: Vec<String> = enabled.iter().filter_map(|client| client.register(id).err().map(|why| format!("{}: {why}", client.name))).collect();
    if failures.len() == enabled.len() {
        return Err(failures.join("\n"));
    }
    start_server(&app)?;
    if failures.is_empty() {
        Ok(())
    } else {
        Err(format!("{} is on, but could not be registered everywhere.\n{}", app.name, failures.join("\n")))
    }
}

/// What to tell someone who turns an app on while no AI client is enabled.
pub fn no_client_hint() -> String {
    let found: Vec<&str> = clients::all().iter().filter(|client| client.detected()).map(|client| client.id).collect();
    let found = if found.is_empty() {
        "none of the clients the hub knows was found on this computer".to_string()
    } else {
        format!("found on this computer: {}", found.join(", "))
    };
    format!("No AI client is enabled, so there is nowhere to register the app. Enable one with `crafthub clients on <client>` ({found}).")
}

/// Turns an app off: removes it from every client and stops its server.
pub fn turn_off(id: &str) -> Result<(), String> {
    let app = Hub::load().app(id).ok_or_else(|| unknown(id))?;
    // Every client the hub knows, not only the enabled ones: nothing of ours is left behind.
    let failures: Vec<String> = clients::all().iter().filter_map(|client| client.unregister(id).err().map(|why| format!("{}: {why}", client.name))).collect();
    if app.port != 0 {
        control::shutdown(app.port, id);
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("\n"))
    }
}

/// Starts registering apps with a client, beginning with the ones that are on. Returns their ids.
pub fn enable_client(id: &str) -> Result<Vec<String>, String> {
    let client = clients::get(id).ok_or_else(|| unknown_client(id))?;
    let hub = Hub::load();
    let on: Vec<String> = hub.apps().into_iter().filter(|app| clients::registered_anywhere(&app.id)).map(|app| app.id).collect();
    let mut config = hub.config;
    let mut ids: Vec<String> = clients::enabled(&config).iter().map(|c| c.id.to_string()).collect();
    if !ids.iter().any(|enabled| enabled == id) {
        ids.push(id.to_string());
    }
    config.clients = Some(ids);
    config.save()?;
    let failures: Vec<String> = on.iter().filter_map(|app| client.register(app).err().map(|why| format!("{app}: {why}"))).collect();
    if failures.is_empty() {
        Ok(on)
    } else {
        Err(failures.join("\n"))
    }
}

/// Stops registering apps with a client and takes ours out of it. Servers of apps that are then
/// registered nowhere are stopped.
pub fn disable_client(id: &str) -> Result<(), String> {
    let client = clients::get(id).ok_or_else(|| unknown_client(id))?;
    let hub = Hub::load();
    let apps = hub.apps();
    let failures: Vec<String> = apps.iter().filter_map(|app| client.unregister(&app.id).err().map(|why| format!("{}: {why}", app.id))).collect();
    let mut config = hub.config;
    config.clients = Some(clients::enabled(&config).iter().map(|c| c.id.to_string()).filter(|enabled| enabled != id).collect());
    config.save()?;
    for app in apps.iter().filter(|app| app.port != 0 && !clients::registered_anywhere(&app.id)) {
        control::shutdown(app.port, &app.id);
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("\n"))
    }
}

fn unknown_client(id: &str) -> String {
    let known: Vec<&str> = clients::all().iter().map(|client| client.id).collect();
    format!("`{id}` is not an AI client the hub knows. It knows: {}.", known.join(", "))
}

/// Restarts the app's server, which also closes any library it has open.
pub fn restart(id: &str) -> Result<(), String> {
    let app = Hub::load().app(id).ok_or_else(|| unknown(id))?;
    control::shutdown(app.port, id);
    start_server(&app)
}

fn was_running(app: &App) -> bool {
    matches!(control::status(app.port), Probe::Up(status) if status.app == app.id)
}

fn edit(id: &str, change: impl FnOnce(&mut AppConfig)) -> Result<(), String> {
    let mut config = Config::load();
    let saved = config.apps.get_mut(id).ok_or_else(|| format!("{id} has not been added yet: `crafthub add {id}`."))?;
    change(saved);
    config.save()
}

/// Moves an app's server to another port. Client sessions follow: they look the port up each time.
pub fn set_port(id: &str, port: u16) -> Result<(), String> {
    let app = Hub::load().app(id).ok_or_else(|| unknown(id))?;
    if app.port == port && app.configured {
        return Ok(());
    }
    if let Some(conflict) = port_conflict(&Config::load(), id, port) {
        return Err(conflict);
    }
    let running = was_running(&app);
    if !app.configured {
        add(id, AddOptions { port: Some(port), ..Default::default() })?;
    } else {
        if running {
            control::shutdown(app.port, id);
        }
        edit(id, |saved| saved.port = port)?;
    }
    match Hub::load().app(id) {
        Some(moved) if running => start_server(&moved),
        _ => Ok(()),
    }
}

/// Sets the folder an app's engine may read and write.
pub fn set_root(id: &str, dir: &str) -> Result<(), String> {
    let app = Hub::load().app(id).ok_or_else(|| unknown(id))?;
    if app.profile.root_flags.is_empty() {
        return Err(format!("{} does not take a file folder.", app.name));
    }
    if !Path::new(dir).is_dir() {
        return Err(format!("There is no folder at {dir}."));
    }
    if !app.configured {
        add(id, AddOptions::default())?;
    }
    edit(id, |saved| saved.root = Some(dir.to_string()))?;
    match Hub::load().app(id) {
        Some(changed) if was_running(&changed) => start_server(&changed),
        _ => Ok(()),
    }
}

/// Forgets an app: turns it off and drops its settings.
pub fn remove(id: &str) -> Result<(), String> {
    turn_off(id)?;
    let mut config = Config::load();
    config.apps.remove(id);
    config.save()
}

/// A full MCP round trip through the app's server. A registered app whose server is waiting for
/// a session is started first, so the check says whether it really comes up.
pub fn check(id: &str) -> Result<CheckOk, String> {
    let app = Hub::load().app(id).ok_or_else(|| unknown(id))?;
    if matches!(control::status(app.port), Probe::Down) && app.installed() && app.configured {
        start_server(&app)?;
    }
    control::check(app.port)
}

pub fn close_library(id: &str) -> Result<(), String> {
    let app = Hub::load().app(id).ok_or_else(|| unknown(id))?;
    control::close_library(app.port)
}

/// Adds and turns on every installed app that isn't in the hub yet.
pub fn setup() -> Vec<(App, Result<(), String>)> {
    let new: Vec<App> = Hub::load().apps().into_iter().filter(|app| app.installed() && !app.configured).collect();
    new.into_iter()
        .map(|app| {
            let outcome = turn_on(&app.id);
            (app, outcome)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_ids_are_plain() {
        assert!(valid_id("chromaforge"));
        assert!(valid_id("craft-anything"));
        assert!(!valid_id("Chroma Forge"));
        assert!(!valid_id("-x"));
        assert!(!valid_id(""));
    }

    #[test]
    fn engine_command_adds_profile_arguments_and_roots() {
        let app = App {
            id: "photocraft".into(),
            name: "PhotoCraft".into(),
            tagline: String::new(),
            origin: Origin::Official,
            color: [0, 0, 0],
            cli: Some(vec!["photocraft-cli".into()]),
            profile: profiles::builtin("photocraft").unwrap().clone(),
            port: 7801,
            configured: true,
            root: Some("/photos".into()),
        };
        assert_eq!(app.engine_command().unwrap(), ["photocraft-cli", "mcp", "--automation-read-root", "/photos", "--automation-write-root", "/photos"]);
    }
}
