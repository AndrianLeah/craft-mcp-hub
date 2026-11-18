//! Registering an app's bridge with AI clients.
//!
//! Every client is told the same thing: a stdio MCP server that runs `crafthub connect <app>`.
//! What differs is where each one keeps its servers:
//!
//! - Claude Code: its user settings, changed through its own `claude mcp` command, because
//!   running sessions rewrite that file all the time;
//! - most others: a JSON file with an object of servers, under a key of the client's own;
//! - Zed: the same, in a settings file that may have comments, which are kept;
//! - Codex: `[mcp_servers.<name>]` tables in a TOML file.
//!
//! The files belong to the clients and may hold other servers' secrets. So they are edited in
//! place (the rest of the file, its key order, indentation and comments are kept), written in one
//! step without loosening who may read them, copied to the hub's backup folder first, and left
//! untouched when they can't be parsed.

use crate::config::{Config, CustomClient};
use crate::{files, paths, proc, timefmt};
use serde::Serialize;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// What a client's settings say about one of our app names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Registration {
    Missing,
    /// Registered to launch this hub's bridge.
    Ours,
    /// Registered by something else; the text describes it.
    Other(String),
}

enum Kind {
    ClaudeCode,
    /// A JSON file with the servers in the object under `key`, each written as `entry` says.
    /// `comments`: the client reads comments and trailing commas in it (JSONC), so a file that has
    /// them is edited too, keeping them; otherwise such a file is left alone.
    Json {
        key: &'static str,
        entry: Entry,
        comments: bool,
    },
    /// A TOML file with one `[<key>.<name>]` table per server.
    Toml {
        key: &'static str,
    },
}

/// What a JSON client's entry for an app holds besides `command` and `args`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Entry {
    /// Nothing else: the client tells a local server by its `command`.
    Plain,
    /// `"type": "stdio"`.
    Typed,
    /// `"type": "local"` and `"tools": ["*"]`, as GitHub Copilot CLI writes its own.
    Copilot,
}

impl Entry {
    fn server(self, command: &str, app: &str) -> Value {
        match self {
            Entry::Plain => json!({ "command": command, "args": ["connect", app] }),
            Entry::Typed => json!({ "type": "stdio", "command": command, "args": ["connect", app] }),
            Entry::Copilot => json!({ "type": "local", "command": command, "args": ["connect", app], "tools": ["*"] }),
        }
    }
}

/// Where a client's settings file is.
enum Location {
    /// A built-in client's file, which depends on the system and on what is installed.
    Known(fn() -> PathBuf),
    At(PathBuf),
}

/// What says a client is on this computer.
#[derive(Clone, Copy)]
enum Found {
    /// The folder its settings file is in: the client makes it when it first runs.
    Folder,
    /// The settings file itself, for a client whose folder another program may make too.
    File,
}

pub struct Client {
    pub id: &'static str,
    pub name: &'static str,
    kind: Kind,
    file: Location,
    found: Found,
    /// Whether the client reads its servers only when it starts.
    pub needs_restart: bool,
}

pub const CLAUDE_CODE: &str = "claude-code";

/// The clients the hub knows by itself. Each one's settings file, the key of its servers, the
/// shape of an entry and whether it needs a restart come from the page cited next to it.
static BUILT_IN: [Client; 12] = [
    Client {
        id: CLAUDE_CODE,
        name: "Claude Code",
        kind: Kind::ClaudeCode,
        file: Location::Known(claude_config_file),
        found: Found::Folder,
        needs_restart: false,
    },
    Client {
        id: "claude-desktop",
        name: "Claude Desktop",
        kind: Kind::Json { key: "mcpServers", entry: Entry::Plain, comments: false },
        file: Location::Known(claude_desktop_file),
        found: Found::Folder,
        needs_restart: true,
    },
    Client {
        id: "cursor",
        name: "Cursor",
        kind: Kind::Json { key: "mcpServers", entry: Entry::Typed, comments: false },
        file: Location::Known(cursor_file),
        found: Found::Folder,
        needs_restart: false,
    },
    Client {
        id: "vscode",
        name: "VS Code",
        kind: Kind::Json { key: "servers", entry: Entry::Typed, comments: false },
        file: Location::Known(vscode_file),
        found: Found::Folder,
        needs_restart: false,
    },
    Client {
        id: "codex",
        name: "Codex",
        kind: Kind::Toml { key: "mcp_servers" },
        file: Location::Known(codex_file),
        found: Found::Folder,
        needs_restart: true,
    },
    // Devin Desktop is the new name of Windsurf (https://docs.devin.ai/desktop/devin-desktop-faq),
    // and it reads the same user file as Devin CLI: https://docs.devin.ai/desktop/cascade/mcp and
    // https://docs.devin.ai/cli/extensibility/mcp/configuration.md (the Windows path). Neither says
    // whether a running app picks up a new server, so it is told to restart.
    Client {
        id: "devin",
        name: "Devin (Windsurf)",
        kind: Kind::Json { key: "mcpServers", entry: Entry::Plain, comments: false },
        file: Location::Known(devin_file),
        found: Found::Folder,
        needs_restart: true,
    },
    // https://geminicli.com/docs/reference/configuration and
    // https://github.com/google-gemini/gemini-cli/blob/main/docs/tools/mcp-server.md: servers are
    // found when the CLI starts. Antigravity uses ~/.gemini too, so only the file says Gemini CLI.
    Client {
        id: "gemini-cli",
        name: "Gemini CLI",
        kind: Kind::Json { key: "mcpServers", entry: Entry::Plain, comments: false },
        file: Location::Known(gemini_file),
        found: Found::File,
        needs_restart: false,
    },
    // https://antigravity.google/docs/mcp: one file for the IDE and the CLI, whose `/mcp` has to
    // reload it.
    Client {
        id: "antigravity",
        name: "Antigravity",
        kind: Kind::Json { key: "mcpServers", entry: Entry::Plain, comments: false },
        file: Location::Known(antigravity_file),
        found: Found::Folder,
        needs_restart: true,
    },
    // https://docs.github.com/en/copilot/how-tos/copilot-cli/customize-copilot/add-mcp-servers, and
    // COPILOT_HOME from https://docs.github.com/en/copilot/how-tos/use-copilot-agents/use-copilot-cli
    Client {
        id: "copilot-cli",
        name: "GitHub Copilot CLI",
        kind: Kind::Json { key: "mcpServers", entry: Entry::Copilot, comments: false },
        file: Location::Known(copilot_file),
        found: Found::Folder,
        needs_restart: false,
    },
    // https://kiro.dev/docs/mcp/configuration: "Changes to MCP configuration apply automatically
    // when you save the file."
    Client {
        id: "kiro",
        name: "Kiro",
        kind: Kind::Json { key: "mcpServers", entry: Entry::Plain, comments: false },
        file: Location::Known(kiro_file),
        found: Found::Folder,
        needs_restart: false,
    },
    // https://lmstudio.ai/docs/app/mcp ("follows Cursor's mcp.json notation"); where the file is
    // and that it is loaded when saved: https://lmstudio.ai/blog/lmstudio-v0.3.17
    Client {
        id: "lm-studio",
        name: "LM Studio",
        kind: Kind::Json { key: "mcpServers", entry: Entry::Plain, comments: false },
        file: Location::Known(lm_studio_file),
        found: Found::Folder,
        needs_restart: false,
    },
    // https://zed.dev/docs/ai/mcp: `context_servers` in the settings file, which Zed starts with
    // comments (assets/settings/initial_user_settings.json in its repository).
    Client {
        id: "zed",
        name: "Zed",
        kind: Kind::Json { key: "context_servers", entry: Entry::Plain, comments: true },
        file: Location::Known(zed_file),
        found: Found::Folder,
        needs_restart: false,
    },
];

pub fn is_built_in(id: &str) -> bool {
    BUILT_IN.iter().any(|client| client.id == id)
}

/// A client the user (or their AI) described to the hub: where it keeps its servers and how.
///
/// The hub hands clients around as `&'static`, so a described one is created once and kept for
/// the life of the process; the same description always gives the same client.
fn custom(id: &str, described: &CustomClient) -> &'static Client {
    static KNOWN: Mutex<Vec<(String, CustomClient, &'static Client)>> = Mutex::new(Vec::new());
    let mut known = KNOWN.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some((_, _, client)) = known.iter().find(|(known_id, known_as, _)| known_id == id && known_as == described) {
        return client;
    }
    let keep = |text: &str| -> &'static str { Box::leak(text.to_string().into_boxed_str()) };
    let key = |default: &str| keep(if described.key.is_empty() { default } else { &described.key });
    let kind = if described.format == "toml" {
        Kind::Toml { key: key("mcp_servers") }
    } else {
        Kind::Json { key: key("mcpServers"), entry: if described.typed { Entry::Typed } else { Entry::Plain }, comments: false }
    };
    let client: &'static Client = Box::leak(Box::new(Client {
        id: keep(id),
        name: keep(if described.name.is_empty() { id } else { &described.name }),
        kind,
        file: Location::At(PathBuf::from(&described.file)),
        found: Found::Folder,
        needs_restart: described.restart,
    }));
    known.push((id.to_string(), described.clone(), client));
    client
}

/// Every client the hub can register apps with: the built-in ones, then the described ones.
pub fn all_in(config: &Config) -> Vec<&'static Client> {
    let described = config.custom_clients.iter().filter(|(id, _)| !is_built_in(id)).map(|(id, described)| custom(id, described));
    BUILT_IN.iter().chain(described).collect()
}

pub fn all() -> Vec<&'static Client> {
    all_in(&Config::load())
}

pub fn get(id: &str) -> Option<&'static Client> {
    all().into_iter().find(|client| client.id == id)
}

/// The clients the hub registers apps with.
pub fn enabled(config: &Config) -> Vec<&'static Client> {
    let all = all_in(config);
    match &config.clients {
        Some(ids) => ids.iter().filter_map(|id| all.iter().copied().find(|client| client.id == id)).collect(),
        // Before clients could be chosen, the hub set up Claude Code.
        None => all.into_iter().filter(|client| client.id == CLAUDE_CODE && client.detected()).collect(),
    }
}

// ---------------------------------------------------------------- where the settings are

/// Claude Code's user settings file.
pub fn claude_config_file() -> PathBuf {
    match std::env::var_os("CLAUDE_CONFIG_DIR").filter(|v| !v.is_empty()) {
        Some(dir) => PathBuf::from(dir).join(".claude.json"),
        None => paths::home_dir().join(".claude.json"),
    }
}

fn claude_desktop_file() -> PathBuf {
    const NAME: &str = "claude_desktop_config.json";
    if cfg!(windows) {
        // The packaged Windows build keeps its settings in Claude-Data\<package>, not in the
        // documented %APPDATA%\Claude.
        let packaged = fs::read_dir(paths::local_data_dir().join("Claude-Data"))
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .filter(|dir| dir.is_dir())
            .max_by_key(|dir| fs::metadata(dir.join(NAME)).and_then(|m| m.modified()).ok());
        if let Some(dir) = packaged {
            return dir.join(NAME);
        }
    }
    paths::app_config_dir().join("Claude").join(NAME)
}

fn cursor_file() -> PathBuf {
    paths::home_dir().join(".cursor").join("mcp.json")
}

fn vscode_file() -> PathBuf {
    paths::app_config_dir().join("Code").join("User").join("mcp.json")
}

fn codex_file() -> PathBuf {
    let home = std::env::var_os("CODEX_HOME").filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or_else(|| paths::home_dir().join(".codex"));
    home.join("config.toml")
}

/// `~/.config/devin/mcp_config.json`, and `%APPDATA%\devin\mcp_config.json` on Windows.
fn devin_file() -> PathBuf {
    let dir = if cfg!(windows) { paths::app_config_dir() } else { paths::home_dir().join(".config") };
    dir.join("devin").join("mcp_config.json")
}

/// `~/.gemini/settings.json`, where `GEMINI_CLI_HOME` stands for the home folder when it is set.
fn gemini_file() -> PathBuf {
    let home = std::env::var_os("GEMINI_CLI_HOME").filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or_else(paths::home_dir);
    home.join(".gemini").join("settings.json")
}

fn antigravity_file() -> PathBuf {
    paths::home_dir().join(".gemini").join("config").join("mcp_config.json")
}

/// `~/.copilot/mcp-config.json`, where `COPILOT_HOME` replaces the whole `~/.copilot` when it is set.
fn copilot_file() -> PathBuf {
    let home = std::env::var_os("COPILOT_HOME").filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or_else(|| paths::home_dir().join(".copilot"));
    home.join("mcp-config.json")
}

fn kiro_file() -> PathBuf {
    paths::home_dir().join(".kiro").join("settings").join("mcp.json")
}

fn lm_studio_file() -> PathBuf {
    paths::home_dir().join(".lmstudio").join("mcp.json")
}

/// Zed's settings file, as its `paths::config_dir` finds it: `%APPDATA%\Zed` on Windows,
/// `~/.config/zed` on macOS, and the XDG config folder (the Flatpak's own, in a Flatpak) on Linux.
fn zed_file() -> PathBuf {
    let dir = if cfg!(windows) {
        paths::app_config_dir().join("Zed")
    } else if cfg!(target_os = "macos") {
        paths::home_dir().join(".config").join("zed")
    } else {
        std::env::var_os("FLATPAK_XDG_CONFIG_HOME").filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or_else(paths::app_config_dir).join("zed")
    };
    dir.join("settings.json")
}

/// The newest file called `name` exactly `depth` folders below `base`.
fn newest_below(base: &Path, depth: usize, name: &str) -> Option<PathBuf> {
    fn walk(dir: &Path, depth: usize, name: &str, found: &mut Vec<PathBuf>) {
        if depth == 0 {
            let file = dir.join(name);
            if file.is_file() {
                found.push(file);
            }
            return;
        }
        for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
            if entry.path().is_dir() {
                walk(&entry.path(), depth - 1, name, found);
            }
        }
    }
    let mut found = Vec::new();
    walk(base, depth, name, &mut found);
    found.into_iter().max_by_key(|file| fs::metadata(file).and_then(|m| m.modified()).ok())
}

/// Claude Code's command-line program: on PATH, in its usual install folders, or the copy that
/// ships inside the Claude desktop app.
pub fn find_claude() -> Option<PathBuf> {
    if let Some(given) = std::env::var_os("CRAFTHUB_CLAUDE").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(given));
    }
    if let Some(found) = paths::find_in_path("claude") {
        return Some(found);
    }
    let home = paths::home_dir();
    let name = paths::exe_name("claude");
    for dir in [home.join(".local").join("bin"), home.join(".claude").join("local")] {
        if dir.join(&name).is_file() {
            return Some(dir.join(&name));
        }
    }
    if cfg!(windows) {
        // %LOCALAPPDATA%\Claude-Data\<package>\claude-code\<version>\<build>\claude.exe
        newest_below(&paths::local_data_dir().join("Claude-Data"), 4, &name)
    } else if cfg!(target_os = "macos") {
        let base = paths::local_data_dir().join("Claude").join("claude-code");
        newest_below(&base, 1, &name).or_else(|| newest_below(&base, 2, &name))
    } else {
        None
    }
}

// ---------------------------------------------------------------- one client

/// What a client must be given to reach the app, in the common `mcpServers` shape.
pub fn entry(app: &str) -> Value {
    json!({ "type": "stdio", "command": paths::hub_exe().to_string_lossy(), "args": ["connect", app], "env": {} })
}

fn same_file(a: &str, b: &Path) -> bool {
    let b_text = b.to_string_lossy();
    let same_text = if cfg!(windows) { a.eq_ignore_ascii_case(&b_text) } else { a == b_text };
    // The same program by another path: a link to it, as in a folder on PATH pointing into the app.
    same_text || matches!((fs::canonicalize(a), fs::canonicalize(b)), (Ok(x), Ok(y)) if x == y)
}

pub fn registration(servers: &Map<String, Value>, app: &str) -> Registration {
    registration_for(servers, app, &paths::hub_exe())
}

fn registration_for(servers: &Map<String, Value>, app: &str, hub: &Path) -> Registration {
    let Some(server) = servers.get(app) else { return Registration::Missing };
    let command = server.get("command").and_then(Value::as_str).unwrap_or("");
    let args: Vec<&str> = server.get("args").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
    if same_file(command, hub) && args == ["connect", app] {
        return Registration::Ours;
    }
    let what = if command.is_empty() { server.get("url").and_then(Value::as_str).unwrap_or("another server") } else { command };
    let name = Path::new(what).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| what.to_string());
    Registration::Other(name)
}

impl Client {
    /// The file the client keeps its servers in.
    pub fn settings_file(&self) -> PathBuf {
        match &self.file {
            Location::Known(find) => find(),
            Location::At(file) => file.clone(),
        }
    }

    /// Whether the client looks installed on this computer.
    pub fn detected(&self) -> bool {
        match (&self.kind, self.found) {
            (Kind::ClaudeCode, _) => self.settings_file().exists() || find_claude().is_some(),
            (_, Found::File) => self.settings_file().is_file(),
            (_, Found::Folder) => self.settings_file().parent().is_some_and(Path::is_dir),
        }
    }

    /// The servers registered in the client, by name, each with its `command` and `args`.
    /// `None` when the settings can't be read right now; an empty map when there are none.
    pub fn registered(&self) -> Option<Map<String, Value>> {
        // The panel asks every couple of seconds: a file is parsed again only when it has changed.
        type Cached = HashMap<PathBuf, (paths::FileStamp, Map<String, Value>)>;
        static CACHE: Mutex<Option<Cached>> = Mutex::new(None);

        let file = self.settings_file();
        let Some(stamp) = paths::file_stamp(&file) else { return (!file.exists()).then(Map::new) };
        let mut cache = CACHE.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let cache = cache.get_or_insert_with(HashMap::new);
        if let Some((cached_stamp, servers)) = cache.get(&file) {
            if *cached_stamp == stamp {
                return Some(servers.clone());
            }
        }
        let text = fs::read_to_string(&file).ok()?;
        let servers = match self.kind {
            Kind::ClaudeCode => json_servers(&text, "mcpServers")?,
            Kind::Json { key, comments: false, .. } => json_servers(&text, key)?,
            Kind::Json { key, comments: true, .. } => jsonc_servers(&text, key)?,
            Kind::Toml { key } => toml_servers(&text, key)?,
        };
        cache.insert(file, (stamp, servers.clone()));
        Some(servers)
    }

    fn registration(&self, app: &str) -> Option<Registration> {
        self.registered().map(|servers| registration(&servers, app))
    }

    /// Registers the app's bridge in the client, replacing whatever was registered under its name.
    pub fn register(&self, app: &str) -> Result<(), String> {
        if self.registration(app) == Some(Registration::Ours) {
            return Ok(());
        }
        let command = paths::hub_exe().to_string_lossy().into_owned();
        match self.kind {
            Kind::ClaudeCode => register_in_claude_code(app),
            Kind::Json { key, entry, comments: false } => self.edit(|text| add_to_json(text, key, app, &command, entry)),
            Kind::Json { key, entry, comments: true } => self.edit(|text| add_to_jsonc(text, key, app, &command, entry)),
            Kind::Toml { key } => self.edit(|text| add_to_toml(text, key, app, &command)),
        }
    }

    /// Removes the app from the client, if this hub registered it there. Someone else's server
    /// under the same name is left alone.
    pub fn unregister(&self, app: &str) -> Result<(), String> {
        if self.registration(app) != Some(Registration::Ours) {
            return Ok(());
        }
        match self.kind {
            Kind::ClaudeCode => {
                let removed = claude(&["mcp", "remove", "-s", "user", app])?;
                if removed.ok {
                    Ok(())
                } else {
                    Err(format!("Claude Code could not remove the server: {}", removed.text))
                }
            }
            Kind::Json { key, comments: false, .. } => self.edit(|text| remove_from_json(text, key, app)),
            Kind::Json { key, comments: true, .. } => self.edit(|text| remove_from_jsonc(text, key, app)),
            Kind::Toml { key } => self.edit(|text| remove_from_toml(text, key, app)),
        }
    }

    /// Changes the client's settings file: reads it, applies `change` to its text, keeps a copy
    /// of the old file, writes the new one.
    fn edit(&self, change: impl FnOnce(&str) -> Result<String, String>) -> Result<(), String> {
        let file = self.settings_file();
        let existing = match fs::read_to_string(&file) {
            Ok(text) => Some(text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(format!("could not read {}: {e}", file.display())),
        };
        let changed = change(existing.as_deref().unwrap_or(""))
            .map_err(|why| format!("{} {why}, so it was left as it is. `crafthub snippet <app>` prints the server to add by hand.", file.display()))?;
        if existing.as_deref() == Some(changed.as_str()) {
            return Ok(());
        }
        if existing.is_some() {
            self.backup(&file);
        }
        files::write_atomic(&file, changed.as_bytes()).map_err(|e| format!("could not write {}: {e}", file.display()))
    }

    /// Keeps a copy of the settings file as it was, and only the five newest copies.
    fn backup(&self, file: &Path) {
        let dir = paths::backup_dir();
        if files::create_private_dir(&dir).is_err() {
            return;
        }
        let prefix = format!("{}-", self.id);
        let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let _ = fs::copy(file, dir.join(format!("{prefix}{}-{name}", timefmt::file_stamp())));
        let mut copies: Vec<PathBuf> = fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with(&prefix)))
            .collect();
        copies.sort();
        for old in copies.iter().rev().skip(5) {
            let _ = fs::remove_file(old);
        }
    }
}

// ---------------------------------------------------------------- Claude Code

fn claude(args: &[&str]) -> Result<proc::Output, String> {
    let program = find_claude().ok_or("Claude Code's command-line program was not found on this computer")?;
    Ok(proc::run(&program, args))
}

fn register_in_claude_code(app: &str) -> Result<(), String> {
    let client = get(CLAUDE_CODE).expect("Claude Code is a known client");
    match client.registered().as_ref().map(|servers| servers.get(app)) {
        Some(None) => {}
        Some(Some(existing)) => {
            // Someone else's server under this name: kept in the backup folder, then replaced.
            let dir = paths::backup_dir();
            let _ = files::create_private_dir(&dir);
            let copy = serde_json::to_string_pretty(&json!({ app: existing })).unwrap_or_default();
            let _ = fs::write(dir.join(format!("{CLAUDE_CODE}-{}-{app}.json", timefmt::file_stamp())), copy);
            claude(&["mcp", "remove", "-s", "user", app])?;
        }
        // The settings couldn't be read: remove blindly, a failure just means there was nothing.
        None => {
            claude(&["mcp", "remove", "-s", "user", app])?;
        }
    }
    let added = claude(&["mcp", "add-json", "-s", "user", app, &entry(app).to_string()])?;
    if added.ok {
        Ok(())
    } else {
        Err(format!("Claude Code did not accept the server: {}", added.text))
    }
}

// ---------------------------------------------------------------- JSON settings

fn without_bom(text: &str) -> &str {
    text.trim_start_matches('\u{feff}')
}

fn json_servers(text: &str, key: &str) -> Option<Map<String, Value>> {
    let text = without_bom(text);
    if text.trim().is_empty() {
        return Some(Map::new());
    }
    let root: Value = serde_json::from_str(text).ok()?;
    Some(root.get(key).and_then(Value::as_object).cloned().unwrap_or_default())
}

/// The whitespace the file indents with: a tab, or some spaces.
fn indent_of(text: &str) -> &str {
    text.lines()
        .find_map(|line| {
            let content = line.trim_start_matches([' ', '\t']);
            let indent = &line[..line.len() - content.len()];
            (!indent.is_empty() && !content.is_empty()).then_some(indent)
        })
        .unwrap_or("  ")
}

/// Applies `change` to the servers under `key`, and writes the file back the way it was written:
/// same key order, indentation and line endings.
fn edit_json(text: &str, key: &str, change: impl FnOnce(&mut Map<String, Value>)) -> Result<String, String> {
    let text = without_bom(text);
    let mut root: Value = if text.trim().is_empty() { json!({}) } else { serde_json::from_str(text).map_err(|e| format!("is not plain JSON ({e})"))? };
    let servers = root
        .as_object_mut()
        .ok_or("does not hold a JSON object")?
        .entry(key)
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| format!("has a `{key}` that is not an object"))?;
    change(servers);

    let mut out = Vec::new();
    let formatter = serde_json::ser::PrettyFormatter::with_indent(indent_of(text).as_bytes());
    root.serialize(&mut serde_json::Serializer::with_formatter(&mut out, formatter)).map_err(|e| e.to_string())?;
    let mut out = String::from_utf8(out).map_err(|e| e.to_string())?;
    if text.is_empty() || text.ends_with('\n') {
        out.push('\n');
    }
    if text.contains("\r\n") {
        out = out.replace('\n', "\r\n");
    }
    Ok(out)
}

fn add_to_json(text: &str, key: &str, app: &str, command: &str, entry: Entry) -> Result<String, String> {
    edit_json(text, key, |servers| {
        servers.insert(app.to_string(), entry.server(command, app));
    })
}

fn remove_from_json(text: &str, key: &str, app: &str) -> Result<String, String> {
    edit_json(text, key, |servers| {
        servers.shift_remove(app);
    })
}

// ---------------------------------------------------------------- JSON with comments (Zed)

/// What a client that reads JSONC accepts: comments and trailing commas, nothing looser.
fn jsonc_options() -> jsonc_parser::ParseOptions {
    jsonc_parser::ParseOptions {
        allow_comments: true,
        allow_trailing_commas: true,
        allow_loose_object_property_names: false,
        allow_missing_commas: false,
        allow_single_quoted_strings: false,
        allow_hexadecimal_numbers: false,
        allow_unary_plus_numbers: false,
        allow_bare_decimal_point_numbers: false,
        allow_non_finite_numbers: false,
        allow_extended_string_escapes: false,
    }
}

fn jsonc_servers(text: &str, key: &str) -> Option<Map<String, Value>> {
    let root: Value = jsonc_parser::parse_to_serde_value(without_bom(text), &jsonc_options()).ok()?;
    match root {
        Value::Null => Some(Map::new()),
        root => Some(root.as_object()?.get(key).and_then(Value::as_object).cloned().unwrap_or_default()),
    }
}

/// Applies `change` to the servers under `key` in the file's syntax tree, so that only the entry
/// it touches is written anew: comments, blank lines, layout and line endings stay as they were.
fn edit_jsonc(text: &str, key: &str, change: impl FnOnce(&jsonc_parser::cst::CstObject)) -> Result<String, String> {
    let text = without_bom(text);
    let root = jsonc_parser::cst::CstRootNode::parse(text, &jsonc_options()).map_err(|e| format!("is not valid JSON with comments ({e})"))?;
    let object = root.object_value_or_create().ok_or("does not hold a JSON object")?;
    let servers = object.object_value_or_create(key).ok_or_else(|| format!("has a `{key}` that is not an object"))?;
    change(&servers);
    let mut out = root.to_string();
    if text.trim().is_empty() {
        out.push('\n');
    }
    Ok(out)
}

/// A JSON value as the syntax tree takes it.
fn cst_value(value: &Value) -> jsonc_parser::cst::CstInputValue {
    use jsonc_parser::cst::CstInputValue as Input;
    match value {
        Value::Null => Input::Null,
        Value::Bool(b) => Input::Bool(*b),
        Value::Number(n) => Input::Number(n.to_string()),
        Value::String(s) => Input::String(s.clone()),
        Value::Array(items) => Input::Array(items.iter().map(cst_value).collect()),
        Value::Object(map) => Input::Object(map.iter().map(|(k, v)| (k.clone(), cst_value(v))).collect()),
    }
}

fn add_to_jsonc(text: &str, key: &str, app: &str, command: &str, entry: Entry) -> Result<String, String> {
    edit_jsonc(text, key, |servers| {
        let server = cst_value(&entry.server(command, app));
        match servers.get(app) {
            Some(existing) => existing.set_value(server),
            None => {
                servers.append(app, server);
            }
        }
    })
}

fn remove_from_jsonc(text: &str, key: &str, app: &str) -> Result<String, String> {
    edit_jsonc(text, key, |servers| {
        if let Some(existing) = servers.get(app) {
            existing.remove();
        }
    })
}

// ---------------------------------------------------------------- TOML settings (Codex)

fn toml_servers(text: &str, key: &str) -> Option<Map<String, Value>> {
    let doc = text.parse::<toml_edit::DocumentMut>().ok()?;
    let mut servers = Map::new();
    if let Some(table) = doc.get(key).and_then(|item| item.as_table_like()) {
        for (name, server) in table.iter() {
            let command = server.get("command").and_then(|c| c.as_str()).unwrap_or("");
            let args: Vec<&str> = server.get("args").and_then(|a| a.as_array()).map(|a| a.iter().filter_map(|v| v.as_str()).collect()).unwrap_or_default();
            servers.insert(name.to_string(), json!({ "command": command, "args": args }));
        }
    }
    Some(servers)
}

/// Applies `change` to the table of servers under `key`; comments and layout of the rest are kept.
fn edit_toml(text: &str, key: &str, change: impl FnOnce(&mut toml_edit::Table)) -> Result<String, String> {
    let mut doc = text.parse::<toml_edit::DocumentMut>().map_err(|e| format!("is not valid TOML ({})", e.to_string().lines().next().unwrap_or("")))?;
    let servers = doc.entry(key).or_insert_with(|| {
        // Implicit: only the `[<key>.<name>]` tables are written, no empty `[<key>]`.
        let mut table = toml_edit::Table::new();
        table.set_implicit(true);
        toml_edit::Item::Table(table)
    });
    change(servers.as_table_mut().ok_or_else(|| format!("has a `{key}` that is not a table"))?);
    Ok(doc.to_string())
}

fn add_to_toml(text: &str, key: &str, app: &str, command: &str) -> Result<String, String> {
    edit_toml(text, key, |servers| {
        let mut args = toml_edit::Array::new();
        args.push("connect");
        args.push(app);
        let mut server = toml_edit::Table::new();
        server.insert("command", toml_edit::value(command));
        server.insert("args", toml_edit::value(args));
        servers.insert(app, toml_edit::Item::Table(server));
    })
}

fn remove_from_toml(text: &str, key: &str, app: &str) -> Result<String, String> {
    edit_toml(text, key, |servers| {
        servers.remove(app);
    })
}

// ---------------------------------------------------------------- all enabled clients at once

/// What the enabled clients have registered, read once for a whole list of apps.
pub struct Snapshot {
    entries: Vec<(&'static Client, Option<Map<String, Value>>)>,
}

/// Where one app stands in the enabled clients.
#[derive(Default)]
pub struct Summary {
    /// Clients where this hub's bridge is registered.
    pub ours: Vec<&'static Client>,
    pub missing: Vec<&'static Client>,
    /// Clients where something else is registered under the app's name, with what.
    pub other: Vec<(&'static Client, String)>,
}

impl Snapshot {
    pub fn take(config: &Config) -> Snapshot {
        Snapshot { entries: enabled(config).into_iter().map(|client| (client, client.registered())).collect() }
    }

    pub fn clients(&self) -> impl Iterator<Item = &'static Client> + '_ {
        self.entries.iter().map(|(client, _)| *client)
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Whether clients are enabled but none of their settings could be read just now.
    pub fn unreadable(&self) -> bool {
        !self.entries.is_empty() && self.entries.iter().all(|(_, servers)| servers.is_none())
    }

    pub fn of(&self, app: &str) -> Summary {
        let mut summary = Summary::default();
        for (client, servers) in &self.entries {
            // A client whose settings can't be read right now is left out rather than guessed at.
            match servers.as_ref().map(|servers| registration(servers, app)) {
                Some(Registration::Ours) => summary.ours.push(*client),
                Some(Registration::Missing) => summary.missing.push(*client),
                Some(Registration::Other(what)) => summary.other.push((*client, what)),
                None => {}
            }
        }
        summary
    }
}

/// The servers for these apps as text to paste into a client's settings by hand: `json` (an
/// `mcpServers` object) or `toml` (`[mcp_servers.<app>]` tables).
pub fn snippet(apps: &[String], format: &str) -> Result<String, String> {
    let command = paths::hub_exe().to_string_lossy().into_owned();
    let mut text = String::new();
    for app in apps {
        text = match format {
            "json" => add_to_json(&text, "mcpServers", app, &command, Entry::Typed)?,
            "toml" => add_to_toml(&text, "mcp_servers", app, &command)?,
            other => return Err(format!("`{other}` is not a format the hub writes: use json or toml.")),
        };
    }
    Ok(text)
}

/// Whether this hub has the app registered in any client it knows, enabled or not.
pub fn registered_anywhere(app: &str) -> bool {
    all().into_iter().any(|client| client.registration(app) == Some(Registration::Ours))
}

#[cfg(test)]
mod tests {
    use super::*;

    const HUB: &str = "/opt/crafthub/crafthub";

    #[test]
    fn adds_to_json_keeping_the_rest_as_it_was() {
        let before = "{\n\t\"inputs\": [],\n\t\"servers\": {\n\t\t\"other\": {\n\t\t\t\"command\": \"npx\"\n\t\t}\n\t}\n}\n";
        let after = add_to_json(before, "servers", "figcraft", HUB, Entry::Typed).unwrap();
        let root: Value = serde_json::from_str(&after).unwrap();
        assert_eq!(root["servers"]["figcraft"], json!({ "type": "stdio", "command": HUB, "args": ["connect", "figcraft"] }));
        assert_eq!(root["servers"]["other"]["command"], "npx");
        // Key order, tab indentation and the final newline are the file's own.
        assert!(after.find("\"inputs\"").unwrap() < after.find("\"servers\"").unwrap());
        assert!(after.contains("\n\t\t\"figcraft\": {\n\t\t\t\"type\": \"stdio\""));
        assert!(after.ends_with("}\n"));
        // Taking it out again gives back the file as it was.
        assert_eq!(remove_from_json(&after, "servers", "figcraft").unwrap(), before);
    }

    #[test]
    fn starts_a_json_file_that_does_not_exist() {
        let after = add_to_json("", "mcpServers", "lightcraft", HUB, Entry::Plain).unwrap();
        let servers = json_servers(&after, "mcpServers").unwrap();
        assert_eq!(registration_for(&servers, "lightcraft", Path::new(HUB)), Registration::Ours);
        assert_eq!(servers["lightcraft"].get("type"), None);
    }

    #[test]
    fn keeps_windows_line_endings() {
        let before = "{\r\n  \"mcpServers\": {}\r\n}\r\n";
        let after = add_to_json(before, "mcpServers", "figcraft", HUB, Entry::Typed).unwrap();
        assert!(after.contains("\r\n") && !after.replace("\r\n", "").contains('\n'));
    }

    #[test]
    fn refuses_json_it_cannot_parse() {
        let with_comment = "{\n  // my servers\n  \"mcpServers\": {}\n}\n";
        assert!(add_to_json(with_comment, "mcpServers", "figcraft", HUB, Entry::Typed).is_err());
        assert!(add_to_json("[]", "mcpServers", "figcraft", HUB, Entry::Typed).is_err());
        assert!(json_servers(with_comment, "mcpServers").is_none());
    }

    #[test]
    fn adds_to_codex_keeping_comments() {
        let before = "# my settings\nmodel = \"gpt\"  # the model\n\n[windows]\nsandbox = \"x\"\n";
        let after = add_to_toml(before, "mcp_servers", "photocraft", "C:\\hub\\crafthub.exe").unwrap();
        assert!(after.starts_with(before));
        assert!(after.contains("[mcp_servers.photocraft]"));
        assert!(!after.contains("[mcp_servers]\n"));
        let servers = toml_servers(&after, "mcp_servers").unwrap();
        assert_eq!(registration_for(&servers, "photocraft", Path::new("C:\\hub\\crafthub.exe")), Registration::Ours);
        assert_eq!(remove_from_toml(&after, "mcp_servers", "photocraft").unwrap(), before);
    }

    #[test]
    fn tells_our_server_from_someone_elses() {
        let servers = json_servers(
            // A path with forward slashes: it splits the same way on every system.
            r#"{"mcpServers":{"figcraft":{"command":"/usr/local/bin/figcraft-cli","args":["mcp"]},"web":{"url":"https://example.com/mcp"}}}"#,
            "mcpServers",
        )
        .unwrap();
        assert_eq!(registration_for(&servers, "figcraft", Path::new(HUB)), Registration::Other("figcraft-cli".to_string()));
        assert_eq!(registration_for(&servers, "lightcraft", Path::new(HUB)), Registration::Missing);
        assert!(matches!(registration_for(&servers, "web", Path::new(HUB)), Registration::Other(_)));
    }

    fn json_shape(id: &str) -> (&'static str, Entry, bool) {
        match BUILT_IN.iter().find(|client| client.id == id).map(|client| &client.kind) {
            Some(Kind::Json { key, entry, comments }) => (key, *entry, *comments),
            _ => panic!("{id} is not a built-in JSON client"),
        }
    }

    #[test]
    fn each_client_gets_the_entry_its_docs_show() {
        assert_eq!(json_shape("claude-desktop"), ("mcpServers", Entry::Plain, false));
        assert_eq!(json_shape("cursor"), ("mcpServers", Entry::Typed, false));
        assert_eq!(json_shape("vscode"), ("servers", Entry::Typed, false));
        assert_eq!(json_shape("devin"), ("mcpServers", Entry::Plain, false));
        assert_eq!(json_shape("gemini-cli"), ("mcpServers", Entry::Plain, false));
        assert_eq!(json_shape("antigravity"), ("mcpServers", Entry::Plain, false));
        assert_eq!(json_shape("copilot-cli"), ("mcpServers", Entry::Copilot, false));
        assert_eq!(json_shape("kiro"), ("mcpServers", Entry::Plain, false));
        assert_eq!(json_shape("lm-studio"), ("mcpServers", Entry::Plain, false));
        assert_eq!(json_shape("zed"), ("context_servers", Entry::Plain, true));

        assert_eq!(Entry::Plain.server(HUB, "figcraft"), json!({ "command": HUB, "args": ["connect", "figcraft"] }));
        assert_eq!(Entry::Typed.server(HUB, "figcraft"), json!({ "type": "stdio", "command": HUB, "args": ["connect", "figcraft"] }));
        assert_eq!(Entry::Copilot.server(HUB, "figcraft"), json!({ "type": "local", "command": HUB, "args": ["connect", "figcraft"], "tools": ["*"] }));
    }

    #[test]
    fn built_in_clients_have_distinct_ids_and_files() {
        let ids: std::collections::HashSet<&str> = BUILT_IN.iter().map(|client| client.id).collect();
        assert_eq!(ids.len(), BUILT_IN.len());
        assert!(BUILT_IN.iter().all(|client| crate::hub::valid_id(client.id)));
        let files: std::collections::HashSet<PathBuf> = BUILT_IN.iter().map(Client::settings_file).collect();
        assert_eq!(files.len(), BUILT_IN.len());
    }

    /// Registers an app in `before` as the built-in client `id` would, checks that every server
    /// already there is kept as it was, and that taking the app out gives back the file as it was.
    fn register_in(id: &str, before: &str) -> String {
        let (key, entry, comments) = json_shape(id);
        let read = |text: &str| if comments { jsonc_servers(text, key) } else { json_servers(text, key) }.expect("the file reads");
        let after = if comments { add_to_jsonc(before, key, "figcraft", HUB, entry) } else { add_to_json(before, key, "figcraft", HUB, entry) }.unwrap();
        let (old, new) = (read(before), read(&after));
        assert!(!old.is_empty());
        for (name, server) in &old {
            assert_eq!(new.get(name), Some(server), "{id}: {name} changed");
        }
        assert_eq!(new["figcraft"], entry.server(HUB, "figcraft"));
        assert_eq!(registration_for(&new, "figcraft", Path::new(HUB)), Registration::Ours);
        let back = if comments { remove_from_jsonc(&after, key, "figcraft") } else { remove_from_json(&after, key, "figcraft") }.unwrap();
        assert_eq!(back, before, "{id}: taking the app out again");
        after
    }

    #[test]
    fn gemini_cli_keeps_its_other_settings() {
        let before = r#"{
  "security": {
    "auth": {
      "selectedType": "oauth-personal"
    }
  },
  "mcpServers": {
    "github": {
      "command": "npx",
      "args": [
        "-y",
        "@modelcontextprotocol/server-github"
      ],
      "env": {
        "GITHUB_TOKEN": "$GITHUB_TOKEN"
      },
      "trust": false
    }
  },
  "ui": {
    "theme": "GitHub"
  }
}
"#;
        let after = register_in("gemini-cli", before);
        assert!(after.find("\"security\"").unwrap() < after.find("\"mcpServers\"").unwrap());
        assert!(after.find("\"mcpServers\"").unwrap() < after.find("\"ui\"").unwrap());
    }

    #[test]
    fn copilot_cli_gets_a_local_server_with_all_its_tools() {
        let before = r#"{
  "mcpServers": {
    "playwright": {
      "type": "local",
      "command": "npx",
      "args": [
        "@playwright/mcp@latest"
      ],
      "env": {},
      "tools": [
        "*"
      ]
    }
  }
}
"#;
        let after = register_in("copilot-cli", before);
        assert!(after.contains("\"figcraft\": {\n      \"type\": \"local\""));
    }

    #[test]
    fn other_clients_keep_their_servers_and_remote_ones() {
        // A remote server, as LM Studio's docs show one.
        let remote = r#"{
  "mcpServers": {
    "hf-mcp-server": {
      "url": "https://huggingface.co/mcp",
      "headers": {
        "Authorization": "Bearer <YOUR_HF_TOKEN>"
      }
    }
  }
}
"#;
        // A local one with the fields Kiro, Devin and Antigravity add to theirs.
        let local = "{\n\t\"mcpServers\": {\n\t\t\"fetch\": {\n\t\t\t\"command\": \"uvx\",\n\t\t\t\"args\": [\n\t\t\t\t\"mcp-server-fetch\"\n\t\t\t],\n\t\t\t\"disabled\": false,\n\t\t\t\"autoApprove\": []\n\t\t}\n\t}\n}\n";
        for id in ["lm-studio", "kiro", "devin", "antigravity"] {
            register_in(id, remote);
            let after = register_in(id, local);
            assert!(after.contains("\n\t\t\"figcraft\": {\n\t\t\t\"command\""), "{id} keeps the tabs");
        }
    }

    const ZED_SETTINGS: &str = r#"// Zed settings
//
// For information on how to configure Zed, see the Zed
// documentation: https://zed.dev/docs/configuring-zed
{
  "ui_font_size": 16,
  "theme": {
    "mode": "system",
    "dark": "One Dark" // the one I like
  },
  /* my servers */
  "context_servers": {
    "github": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-github"],
      "env": {},
    },
  },
}
"#;

    #[test]
    fn zed_keeps_its_comments_and_layout() {
        let after = register_in("zed", ZED_SETTINGS);
        for kept in ["// Zed settings\n", "// the one I like", "/* my servers */", "\"args\": [\"-y\", \"@modelcontextprotocol/server-github\"],"] {
            assert!(after.contains(kept), "{kept:?} was lost:\n{after}");
        }
        // Only the new entry is new: the file before it is word for word the same.
        let start = ZED_SETTINGS.find("    },\n  },\n}").unwrap();
        assert!(after.starts_with(&ZED_SETTINGS[..start]));
        assert!(after.ends_with("}\n"));
        // Registering again changes nothing.
        let (key, entry, _) = json_shape("zed");
        assert_eq!(add_to_jsonc(&after, key, "figcraft", HUB, entry).unwrap(), after);
    }

    #[test]
    fn zed_settings_without_servers_get_some() {
        let before = "// Zed settings\r\n{\r\n  \"vim_mode\": true\r\n}\r\n";
        let after = add_to_jsonc(before, "context_servers", "figcraft", HUB, Entry::Plain).unwrap();
        assert!(after.starts_with("// Zed settings\r\n{\r\n  \"vim_mode\": true,\r\n  \"context_servers\": {"));
        assert!(!after.replace("\r\n", "").contains('\n'), "the file's line endings are kept");
        let servers = jsonc_servers(&after, "context_servers").unwrap();
        assert_eq!(registration_for(&servers, "figcraft", Path::new(HUB)), Registration::Ours);
        // A file that isn't there yet.
        let new = add_to_jsonc("", "context_servers", "figcraft", HUB, Entry::Plain).unwrap();
        assert_eq!(registration_for(&jsonc_servers(&new, "context_servers").unwrap(), "figcraft", Path::new(HUB)), Registration::Ours);
        assert!(jsonc_servers("", "context_servers").unwrap().is_empty());
    }

    #[test]
    fn zed_settings_it_cannot_read_are_left_alone() {
        for text in ["{ 'theme': 'One Dark' }", "[]", "{ \"context_servers\": [] }", "{ \"ui_font_size\": 16"] {
            assert!(add_to_jsonc(text, "context_servers", "figcraft", HUB, Entry::Plain).is_err(), "{text}");
        }
        assert!(jsonc_servers("{ 'theme': 'One Dark' }", "context_servers").is_none());
    }

    #[test]
    fn a_client_is_found_by_its_folder_or_by_its_file() {
        let dir = std::env::temp_dir().join(format!("crafthub-found-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("settings.json");
        let client = |found| Client {
            id: "test",
            name: "Test",
            kind: Kind::Json { key: "mcpServers", entry: Entry::Plain, comments: false },
            file: Location::At(file.clone()),
            found,
            needs_restart: false,
        };
        assert!(client(Found::Folder).detected());
        assert!(!client(Found::File).detected());
        fs::write(&file, "{}").unwrap();
        assert!(client(Found::File).detected());
        let _ = fs::remove_dir_all(&dir);
    }
}
