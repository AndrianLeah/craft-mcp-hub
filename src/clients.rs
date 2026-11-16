//! Registering an app's bridge with AI clients.
//!
//! Every client is told the same thing: a stdio MCP server that runs `crafthub connect <app>`.
//! What differs is where each one keeps its servers:
//!
//! - Claude Code: its user settings, changed through its own `claude mcp` command, because
//!   running sessions rewrite that file all the time;
//! - Claude Desktop, Cursor, VS Code: a JSON file with an object of servers;
//! - Codex: `[mcp_servers.<name>]` tables in a TOML file.
//!
//! The files belong to the clients and may hold other servers' secrets. So they are edited in
//! place (the rest of the file, its key order, indentation and comments are kept), written in one
//! step without loosening who may read them, copied to the hub's backup folder first, and left
//! untouched when they can't be parsed.

use crate::config::Config;
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
    /// A JSON file with the servers in the object under `key`; `typed` entries carry `"type": "stdio"`.
    Json {
        key: &'static str,
        typed: bool,
    },
    Codex,
}

pub struct Client {
    pub id: &'static str,
    pub name: &'static str,
    kind: Kind,
    file: fn() -> PathBuf,
    /// Whether the client reads its servers only when it starts.
    pub needs_restart: bool,
}

pub const CLAUDE_CODE: &str = "claude-code";

static CLIENTS: [Client; 5] = [
    Client { id: CLAUDE_CODE, name: "Claude Code", kind: Kind::ClaudeCode, file: claude_config_file, needs_restart: false },
    Client {
        id: "claude-desktop",
        name: "Claude Desktop",
        kind: Kind::Json { key: "mcpServers", typed: false },
        file: claude_desktop_file,
        needs_restart: true,
    },
    Client { id: "cursor", name: "Cursor", kind: Kind::Json { key: "mcpServers", typed: true }, file: cursor_file, needs_restart: false },
    Client { id: "vscode", name: "VS Code", kind: Kind::Json { key: "servers", typed: true }, file: vscode_file, needs_restart: false },
    Client { id: "codex", name: "Codex", kind: Kind::Codex, file: codex_file, needs_restart: true },
];

pub fn all() -> &'static [Client] {
    &CLIENTS
}

pub fn get(id: &str) -> Option<&'static Client> {
    CLIENTS.iter().find(|client| client.id == id)
}

/// The clients the hub registers apps with.
pub fn enabled(config: &Config) -> Vec<&'static Client> {
    match &config.clients {
        Some(ids) => ids.iter().filter_map(|id| get(id)).collect(),
        // Before clients could be chosen, the hub set up Claude Code.
        None => CLIENTS.iter().filter(|client| client.id == CLAUDE_CODE && client.detected()).collect(),
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
    let b = b.to_string_lossy();
    if cfg!(windows) {
        a.eq_ignore_ascii_case(&b)
    } else {
        a == b
    }
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
        (self.file)()
    }

    /// Whether the client looks installed on this computer.
    pub fn detected(&self) -> bool {
        match self.kind {
            Kind::ClaudeCode => self.settings_file().exists() || find_claude().is_some(),
            _ => self.settings_file().parent().is_some_and(Path::is_dir),
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
            Kind::Json { key, .. } => json_servers(&text, key)?,
            Kind::Codex => codex_servers(&text)?,
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
            Kind::Json { key, typed } => self.edit(|text| add_to_json(text, key, app, &command, typed)),
            Kind::Codex => self.edit(|text| add_to_codex(text, app, &command)),
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
            Kind::Json { key, .. } => self.edit(|text| remove_from_json(text, key, app)),
            Kind::Codex => self.edit(|text| remove_from_codex(text, app)),
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

fn add_to_json(text: &str, key: &str, app: &str, command: &str, typed: bool) -> Result<String, String> {
    edit_json(text, key, |servers| {
        let server = if typed {
            json!({ "type": "stdio", "command": command, "args": ["connect", app] })
        } else {
            json!({ "command": command, "args": ["connect", app] })
        };
        servers.insert(app.to_string(), server);
    })
}

fn remove_from_json(text: &str, key: &str, app: &str) -> Result<String, String> {
    edit_json(text, key, |servers| {
        servers.shift_remove(app);
    })
}

// ---------------------------------------------------------------- Codex settings

fn codex_servers(text: &str) -> Option<Map<String, Value>> {
    let doc = text.parse::<toml_edit::DocumentMut>().ok()?;
    let mut servers = Map::new();
    if let Some(table) = doc.get("mcp_servers").and_then(|item| item.as_table_like()) {
        for (name, server) in table.iter() {
            let command = server.get("command").and_then(|c| c.as_str()).unwrap_or("");
            let args: Vec<&str> = server.get("args").and_then(|a| a.as_array()).map(|a| a.iter().filter_map(|v| v.as_str()).collect()).unwrap_or_default();
            servers.insert(name.to_string(), json!({ "command": command, "args": args }));
        }
    }
    Some(servers)
}

/// Applies `change` to the `mcp_servers` table; comments and layout of the rest are kept.
fn edit_codex(text: &str, change: impl FnOnce(&mut toml_edit::Table)) -> Result<String, String> {
    let mut doc = text.parse::<toml_edit::DocumentMut>().map_err(|e| format!("is not valid TOML ({})", e.to_string().lines().next().unwrap_or("")))?;
    let servers = doc.entry("mcp_servers").or_insert_with(|| {
        // Implicit: only the `[mcp_servers.<name>]` tables are written, no empty `[mcp_servers]`.
        let mut table = toml_edit::Table::new();
        table.set_implicit(true);
        toml_edit::Item::Table(table)
    });
    change(servers.as_table_mut().ok_or("has an `mcp_servers` that is not a table")?);
    Ok(doc.to_string())
}

fn add_to_codex(text: &str, app: &str, command: &str) -> Result<String, String> {
    edit_codex(text, |servers| {
        let mut args = toml_edit::Array::new();
        args.push("connect");
        args.push(app);
        let mut server = toml_edit::Table::new();
        server.insert("command", toml_edit::value(command));
        server.insert("args", toml_edit::value(args));
        servers.insert(app, toml_edit::Item::Table(server));
    })
}

fn remove_from_codex(text: &str, app: &str) -> Result<String, String> {
    edit_codex(text, |servers| {
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

/// Whether this hub has the app registered in any client it knows, enabled or not.
pub fn registered_anywhere(app: &str) -> bool {
    CLIENTS.iter().any(|client| client.registration(app) == Some(Registration::Ours))
}

#[cfg(test)]
mod tests {
    use super::*;

    const HUB: &str = "/opt/crafthub/crafthub";

    #[test]
    fn adds_to_json_keeping_the_rest_as_it_was() {
        let before = "{\n\t\"inputs\": [],\n\t\"servers\": {\n\t\t\"other\": {\n\t\t\t\"command\": \"npx\"\n\t\t}\n\t}\n}\n";
        let after = add_to_json(before, "servers", "figcraft", HUB, true).unwrap();
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
        let after = add_to_json("", "mcpServers", "lightcraft", HUB, false).unwrap();
        let servers = json_servers(&after, "mcpServers").unwrap();
        assert_eq!(registration_for(&servers, "lightcraft", Path::new(HUB)), Registration::Ours);
        assert_eq!(servers["lightcraft"].get("type"), None);
    }

    #[test]
    fn keeps_windows_line_endings() {
        let before = "{\r\n  \"mcpServers\": {}\r\n}\r\n";
        let after = add_to_json(before, "mcpServers", "figcraft", HUB, true).unwrap();
        assert!(after.contains("\r\n") && !after.replace("\r\n", "").contains('\n'));
    }

    #[test]
    fn refuses_json_it_cannot_parse() {
        let with_comment = "{\n  // my servers\n  \"mcpServers\": {}\n}\n";
        assert!(add_to_json(with_comment, "mcpServers", "figcraft", HUB, true).is_err());
        assert!(add_to_json("[]", "mcpServers", "figcraft", HUB, true).is_err());
        assert!(json_servers(with_comment, "mcpServers").is_none());
    }

    #[test]
    fn adds_to_codex_keeping_comments() {
        let before = "# my settings\nmodel = \"gpt\"  # the model\n\n[windows]\nsandbox = \"x\"\n";
        let after = add_to_codex(before, "photocraft", "C:\\hub\\crafthub.exe").unwrap();
        assert!(after.starts_with(before));
        assert!(after.contains("[mcp_servers.photocraft]"));
        assert!(!after.contains("[mcp_servers]\n"));
        let servers = codex_servers(&after).unwrap();
        assert_eq!(registration_for(&servers, "photocraft", Path::new("C:\\hub\\crafthub.exe")), Registration::Ours);
        assert_eq!(remove_from_codex(&after, "photocraft").unwrap(), before);
    }

    #[test]
    fn tells_our_server_from_someone_elses() {
        let servers = json_servers(
            r#"{"mcpServers":{"figcraft":{"command":"C:\\Program Files\\FigCraft\\figcraft-cli.exe","args":["mcp"]},"web":{"url":"https://example.com/mcp"}}}"#,
            "mcpServers",
        )
        .unwrap();
        assert_eq!(registration_for(&servers, "figcraft", Path::new(HUB)), Registration::Other("figcraft-cli.exe".to_string()));
        assert_eq!(registration_for(&servers, "lightcraft", Path::new(HUB)), Registration::Missing);
        assert!(matches!(registration_for(&servers, "web", Path::new(HUB)), Registration::Other(_)));
    }
}
