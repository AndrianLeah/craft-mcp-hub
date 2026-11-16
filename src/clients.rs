//! Registering an app's bridge with AI clients. For now: Claude Code (user scope).
//!
//! Registration goes through Claude Code's own `claude mcp` command rather than by editing its
//! settings file, which running sessions rewrite all the time.

use crate::{paths, proc, timefmt};
use serde_json::{json, Map, Value};
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

/// Claude Code's user settings file.
pub fn claude_config_file() -> PathBuf {
    match std::env::var_os("CLAUDE_CONFIG_DIR").filter(|v| !v.is_empty()) {
        Some(dir) => PathBuf::from(dir).join(".claude.json"),
        None => paths::home_dir().join(".claude.json"),
    }
}

/// The MCP servers registered in Claude Code's user settings. `None` when the file can't be read
/// right now (Claude rewrites it often); an empty map when there are none.
pub fn registered() -> Option<Map<String, Value>> {
    // The panel asks every couple of seconds and the file is large: it is parsed again only when
    // it has changed.
    type Cached = (PathBuf, paths::FileStamp, Map<String, Value>);
    static CACHE: Mutex<Option<Cached>> = Mutex::new(None);

    let file = claude_config_file();
    let Some(stamp) = paths::file_stamp(&file) else { return (!file.exists()).then(Map::new) };
    let mut cache = CACHE.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some((cached_file, cached_stamp, servers)) = cache.as_ref() {
        if *cached_file == file && *cached_stamp == stamp {
            return Some(servers.clone());
        }
    }
    let settings: Value = serde_json::from_str(&fs::read_to_string(&file).ok()?).ok()?;
    let servers = settings.get("mcpServers").and_then(Value::as_object).cloned().unwrap_or_default();
    *cache = Some((file, stamp, servers.clone()));
    Some(servers)
}

/// What a client must be given to reach the app: a stdio server running `crafthub connect <app>`.
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
    let Some(server) = servers.get(app) else { return Registration::Missing };
    let command = server.get("command").and_then(Value::as_str).unwrap_or("");
    let args: Vec<&str> = server.get("args").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
    if same_file(command, &paths::hub_exe()) && args == ["connect", app] {
        return Registration::Ours;
    }
    let what = if command.is_empty() { server.get("url").and_then(Value::as_str).unwrap_or("another server") } else { command };
    let name = Path::new(what).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| what.to_string());
    Registration::Other(name)
}

/// Keeps a copy of a registration we are about to replace.
fn backup(app: &str, server: &Value) {
    let dir = paths::backup_dir();
    let _ = fs::create_dir_all(&dir);
    let file = dir.join(format!("claude-code-{app}-{}.json", timefmt::file_stamp()));
    let _ = fs::write(file, serde_json::to_string_pretty(&json!({ app: server })).unwrap_or_default());
}

fn claude(args: &[&str]) -> Result<proc::Output, String> {
    let program = find_claude().ok_or("Claude Code was not found on this computer")?;
    Ok(proc::run(&program, args))
}

/// Registers the app's bridge in Claude Code, replacing whatever was registered under its name.
pub fn register(app: &str) -> Result<(), String> {
    let servers = registered();
    if servers.as_ref().is_some_and(|s| registration(s, app) == Registration::Ours) {
        return Ok(());
    }
    if find_claude().is_none() {
        return Err(format!(
            "Claude Code was not found on this computer. To use {app} from another MCP client, add this server to it by hand:\n{}",
            serde_json::to_string_pretty(&json!({ app: entry(app) })).unwrap_or_default()
        ));
    }
    match servers.as_ref().map(|s| s.get(app)) {
        Some(None) => {}
        Some(Some(existing)) => {
            backup(app, existing);
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

/// Removes the app from Claude Code. Fine when it wasn't registered.
pub fn unregister(app: &str) -> Result<(), String> {
    let servers = registered();
    match servers.as_ref().map(|s| s.get(app)) {
        Some(None) => return Ok(()),
        Some(Some(existing)) if registration(servers.as_ref().unwrap(), app) != Registration::Ours => backup(app, existing),
        _ => {}
    }
    let removed = claude(&["mcp", "remove", "-s", "user", app])?;
    if removed.ok || servers.is_none() {
        Ok(())
    } else {
        Err(format!("Claude Code could not remove the server: {}", removed.text))
    }
}
