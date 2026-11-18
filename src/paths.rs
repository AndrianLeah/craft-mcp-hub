//! Where things live on each system.

use std::path::PathBuf;

pub fn home_dir() -> PathBuf {
    std::env::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

/// The hub's own folder (settings, token, logs). It is in the home folder rather than the platform's
/// app-data folder on purpose: Claude's desktop app on Windows is an MSIX package, and what its child
/// processes write under AppData lands in a private copy that other programs can't see.
pub fn hub_home() -> PathBuf {
    match std::env::var_os("CRAFTHUB_HOME") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => home_dir().join(".crafthub"),
    }
}

pub fn config_file() -> PathBuf {
    hub_home().join("config.toml")
}

pub fn token_file() -> PathBuf {
    hub_home().join("token")
}

pub fn log_dir() -> PathBuf {
    hub_home().join("logs")
}

pub fn backup_dir() -> PathBuf {
    hub_home().join("backup")
}

pub fn catalog_cache() -> PathBuf {
    hub_home().join("catalog.toml")
}

/// Where the hub keeps the command-line tools it downloaded itself, one folder per app. On macOS
/// the ArtCraft launcher installs only the desktop app, so the hub fetches the tool that goes with it.
pub fn tools_dir() -> PathBuf {
    hub_home().join("tools")
}

fn env_dir(var: &str) -> Option<PathBuf> {
    std::env::var_os(var).filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// The per-user settings folder, where the apps keep their own settings.
pub fn app_config_dir() -> PathBuf {
    if cfg!(windows) {
        env_dir("APPDATA").unwrap_or_else(|| home_dir().join("AppData").join("Roaming"))
    } else if cfg!(target_os = "macos") {
        home_dir().join("Library").join("Application Support")
    } else {
        env_dir("XDG_CONFIG_HOME").unwrap_or_else(|| home_dir().join(".config"))
    }
}

/// The per-user data folder.
pub fn local_data_dir() -> PathBuf {
    if cfg!(windows) {
        env_dir("LOCALAPPDATA").unwrap_or_else(|| home_dir().join("AppData").join("Local"))
    } else if cfg!(target_os = "macos") {
        home_dir().join("Library").join("Application Support")
    } else {
        env_dir("XDG_DATA_HOME").unwrap_or_else(|| home_dir().join(".local").join("share"))
    }
}

/// The per-user cache folder.
pub fn cache_dir() -> PathBuf {
    if cfg!(windows) {
        local_data_dir()
    } else if cfg!(target_os = "macos") {
        home_dir().join("Library").join("Caches")
    } else {
        env_dir("XDG_CACHE_HOME").unwrap_or_else(|| home_dir().join(".cache"))
    }
}

/// When a file was last written and how big it is: enough to tell whether it has changed.
pub type FileStamp = (std::time::SystemTime, u64);

pub fn file_stamp(file: &std::path::Path) -> Option<FileStamp> {
    let meta = std::fs::metadata(file).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

pub fn exe_name(stem: &str) -> String {
    if cfg!(windows) {
        format!("{stem}.exe")
    } else {
        stem.to_string()
    }
}

fn sibling(stem: &str) -> PathBuf {
    let me = std::env::current_exe().unwrap_or_default();
    let name = exe_name(stem);
    let is_me = me.file_name().is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(&name));
    if is_me {
        me
    } else {
        me.with_file_name(name)
    }
}

/// The command-line binary: it runs the servers and is what AI clients launch.
pub fn hub_exe() -> PathBuf {
    sibling("crafthub")
}

/// The control panel binary, installed next to the command-line one. Inside the macOS app it is
/// named after the app.
pub fn panel_exe() -> PathBuf {
    let panel = sibling("crafthub-panel");
    let in_app = panel.with_file_name(crate::macapp::APP_NAME);
    if cfg!(target_os = "macos") && !panel.is_file() && in_app.is_file() {
        in_app
    } else {
        panel
    }
}

/// A program on PATH. On Windows `name` is tried with the usual executable extensions.
pub fn find_in_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let names: Vec<String> =
        if cfg!(windows) && !name.contains('.') { ["exe", "cmd", "bat"].iter().map(|ext| format!("{name}.{ext}")).collect() } else { vec![name.to_string()] };
    for dir in std::env::split_paths(&path) {
        for name in &names {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}
