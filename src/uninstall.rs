//! Taking the hub off this computer: what `crafthub uninstall` and the panel's "Uninstall…" do.
//!
//! 1. Every app is turned off: its server stops and it is taken out of every AI client.
//! 2. The macOS app, "Craft MCP Hub", is deleted from Applications.
//! 3. `crafthub` and `crafthub-panel` are deleted from the folder the install script put them in.
//! 4. The hub's folder (`~/.crafthub`) is emptied of what the hub keeps there, then deleted.
//!
//! The ArtCraft apps themselves are left alone. Only files the hub itself makes are deleted, by
//! name, so a `CRAFTHUB_HOME` pointing at a folder holding other things loses nothing else.

use crate::hub::{self, Hub};
use crate::{macapp, paths};
use std::fs;
use std::path::{Path, PathBuf};

/// What the hub keeps in its folder.
const HUB_FILES: [&str; 7] = ["config.toml", "token", "logs", "backup", "catalog.toml", "tools", "bin"];
const PROGRAMS: [&str; 2] = ["crafthub", "crafthub-panel"];

/// The folder the install script puts the programs in on macOS and Linux.
fn install_dir() -> PathBuf {
    match std::env::var_os("CRAFTHUB_INSTALL_DIR") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => paths::home_dir().join(".local/bin"),
    }
}

/// The installed programs: in the install folder, as files or as links into the macOS app.
fn installed_programs() -> Vec<PathBuf> {
    if cfg!(windows) {
        // On Windows they are in the hub's folder, which is deleted as a whole.
        return Vec::new();
    }
    let dir = install_dir();
    PROGRAMS.iter().map(|name| dir.join(name)).filter(|file| fs::symlink_metadata(file).is_ok()).collect()
}

/// The Start menu shortcut the Windows install script makes.
fn start_menu_shortcut() -> Option<PathBuf> {
    let appdata = std::env::var_os("APPDATA").filter(|_| cfg!(windows))?;
    let shortcut = PathBuf::from(appdata).join("Microsoft/Windows/Start Menu/Programs/Craft MCP Hub.lnk");
    shortcut.is_file().then_some(shortcut)
}

/// Whether the hub's folder holds anything the hub put there.
fn hub_files_present(home: &Path) -> bool {
    HUB_FILES.iter().any(|name| fs::symlink_metadata(home.join(name)).is_ok())
}

/// What uninstalling would do, one line each, for people to read before they agree.
pub fn plan() -> Vec<String> {
    let mut steps = Vec::new();
    let on: Vec<String> = Hub::load().apps().into_iter().filter(|app| app.configured).map(|app| app.name).collect();
    if !on.is_empty() {
        steps.push(format!("Turn off {} and take them out of every AI client", on.join(", ")));
    }
    for bundle in macapp::all_installed() {
        steps.push(format!("Delete {}", bundle.display()));
    }
    for program in installed_programs() {
        steps.push(format!("Delete {}", program.display()));
    }
    if let Some(shortcut) = start_menu_shortcut() {
        steps.push(format!("Delete the Start menu shortcut ({})", shortcut.display()));
    }
    let home = paths::hub_home();
    if hub_files_present(&home) {
        steps.push(format!("Delete {} (settings, access token, logs, backups, downloaded tools)", home.display()));
    }
    steps
}

/// Removes a file, a link or a folder, whichever `path` is. A missing one is fine.
fn remove(path: &Path) -> Result<(), String> {
    let Ok(meta) = fs::symlink_metadata(path) else { return Ok(()) };
    let result = if meta.is_dir() { fs::remove_dir_all(path) } else { fs::remove_file(path) };
    result.map_err(|e| format!("could not delete {}: {e}", path.display()))
}

/// Uninstalls the hub. Returns what was done and what could not be, step by step; it carries on
/// past a step that fails, so as much as possible is gone.
pub fn run() -> Vec<(String, Result<(), String>)> {
    let mut done = Vec::new();

    for app in Hub::load().apps().into_iter().filter(|app| app.configured) {
        done.push((format!("{} turned off", app.name), hub::turn_off(&app.id)));
    }
    for bundle in macapp::all_installed() {
        done.push((format!("{} deleted", bundle.display()), remove(&bundle)));
    }
    for program in installed_programs() {
        done.push((format!("{} deleted", program.display()), remove(&program)));
    }
    if let Some(shortcut) = start_menu_shortcut() {
        done.push(("Start menu shortcut deleted".to_string(), remove(&shortcut)));
    }

    let home = paths::hub_home();
    if hub_files_present(&home) {
        let mut outcome = Ok(());
        for name in HUB_FILES {
            let path = home.join(name);
            // On Windows the program running this is in `bin`, and a running program can't be
            // deleted: what is left is said below.
            if cfg!(windows) && name == "bin" {
                continue;
            }
            if let Err(why) = remove(&path) {
                outcome = Err(why);
            }
        }
        // Half-written files a crash left behind.
        for entry in fs::read_dir(&home).into_iter().flatten().flatten() {
            if entry.file_name().to_string_lossy().ends_with(".crafthub-tmp") {
                let _ = fs::remove_file(entry.path());
            }
        }
        // The folder itself only when nothing else is in it.
        let _ = fs::remove_dir(&home);
        let what = if home.exists() && outcome.is_ok() {
            format!("The hub's files deleted from {} (other files in it were kept)", home.display())
        } else {
            format!("{} deleted", home.display())
        };
        done.push((what, outcome));
    }
    done
}

/// What is left to do by hand after `run`, if anything.
pub fn left_over() -> Option<String> {
    let bin = paths::hub_home().join("bin");
    (cfg!(windows) && bin.exists()).then(|| {
        format!(
            "Delete {} once this window is closed, and take it out of your PATH (Settings → \"Edit environment variables for your account\").",
            bin.display()
        )
    })
}
