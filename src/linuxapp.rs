//! The control panel in the Linux desktop's app menu and search, with its name and icon, as the
//! Windows install script gives it a Start menu shortcut and macOS gets the app in Applications.
//!
//! `crafthub install-app` writes a desktop file, `~/.local/share/applications/craft-mcp-hub.desktop`,
//! that starts `crafthub-panel` where it is installed, and the icon in each size into
//! `~/.local/share/icons/hicolor` (both under `$XDG_DATA_HOME` when it is set). The panel's window
//! takes the same name as its app id (Wayland) and class (X11): that is how the desktop knows the
//! window belongs to the entry, and shows the entry's icon for it.

use crate::macapp::APP_NAME;
use crate::{paths, proc};
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

/// The name of the desktop file and of the icon, and the panel window's app id.
pub const APP_ID: &str = "craft-mcp-hub";

/// The same words as the Windows Start menu shortcut's.
const COMMENT: &str = "Turn the MCP servers of the ArtCraft apps on and off";

/// The icon sizes installed, and their pictures as `assets/icons/linux/hicolor` has them. The
/// pictures are built into the Linux program only, and only the code that installs them uses them:
/// the panel, which only needs the sizes to uninstall, doesn't carry them.
macro_rules! icons {
    ($($size:literal),* $(,)?) => {
        const ICON_SIZES: &[&str] = &[$($size),*];
        #[cfg(target_os = "linux")]
        const ICON_PICTURES: &[&[u8]] = &[$(include_bytes!(concat!("../assets/icons/linux/hicolor/", $size, "/apps/craft-mcp-hub.png"))),*];
        #[cfg(not(target_os = "linux"))]
        const ICON_PICTURES: &[&[u8]] = &[];
    };
}

// The 1024 px picture is left out (no menu shows an icon that big), and so is the SVG: some desktops
// would draw it at every size, in place of the pictures redrawn for the small sizes.
icons!["16x16", "22x22", "24x24", "32x32", "48x48", "64x64", "96x96", "128x128", "256x256", "512x512"];

fn desktop_file_in(data: &Path) -> PathBuf {
    data.join("applications").join(format!("{APP_ID}.desktop"))
}

fn hicolor_in(data: &Path) -> PathBuf {
    data.join("icons").join("hicolor")
}

fn icon_file_in(data: &Path, size: &str) -> PathBuf {
    hicolor_in(data).join(size).join("apps").join(format!("{APP_ID}.png"))
}

/// The desktop file, where the desktop looks for it.
pub fn desktop_file() -> PathBuf {
    desktop_file_in(&paths::local_data_dir())
}

/// Where the icons go, for people to read: `…/icons/hicolor/*/apps/craft-mcp-hub.png`.
pub fn icon_files() -> String {
    hicolor_in(&paths::local_data_dir()).join("*").join("apps").join(format!("{APP_ID}.png")).display().to_string()
}

/// A value as a desktop file stores it: backslashes doubled, and the characters that would end or
/// break the line written as escapes.
fn escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('\n', "\\n").replace('\t', "\\t").replace('\r', "\\r")
}

/// One argument of `Exec`, quoted as the Desktop Entry Specification asks: in double quotes, with
/// `"`, `` ` ``, `$` and `\` behind a backslash and `%` doubled. Then escaped like any other value,
/// so a backslash ends up written as four.
fn exec_argument(argument: &str) -> String {
    let mut quoted = String::from('"');
    for c in argument.chars() {
        match c {
            '"' | '`' | '$' | '\\' => {
                quoted.push('\\');
                quoted.push(c);
            }
            '%' => quoted.push_str("%%"),
            _ => quoted.push(c),
        }
    }
    quoted.push('"');
    escape(&quoted)
}

/// The desktop file that opens the control panel at `panel`.
fn desktop_entry(panel: &Path) -> Result<String, String> {
    let panel = panel.to_str().ok_or_else(|| format!("The control panel's path ({}) is not UTF-8, which a desktop file needs.", panel.display()))?;
    // TryExec hides the entry when the program is gone, for example a build folder that was cleaned.
    // One main category, as the Mac app's (developer tools): with two, menus sorted by category list
    // the entry twice.
    Ok(format!(
        "[Desktop Entry]
Type=Application
Name={APP_NAME}
Comment={COMMENT}
TryExec={try_exec}
Exec={exec}
Icon={APP_ID}
Terminal=false
Categories=Development;
Keywords=crafthub;MCP;ArtCraft;
StartupWMClass={APP_ID}
",
        try_exec = escape(panel),
        exec = exec_argument(panel),
    ))
}

/// Writes a file in one step, since the desktop may read it at any moment.
fn write(file: &Path, content: &[u8]) -> Result<(), String> {
    let dir = file.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(dir).map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    let name = file.file_name().unwrap_or_default().to_string_lossy();
    let temporary = dir.join(format!(".{name}.new-{}", std::process::id()));
    let written = fs::write(&temporary, content).and_then(|()| fs::rename(&temporary, file));
    if written.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    written.map_err(|e| format!("could not write {}: {e}", file.display()))
}

/// Tells the desktop about the icons now. A theme folder whose date changes is looked at again. An
/// icon cache is brought up to date only when there is one already: one made here would go stale
/// when another program adds its icon without updating it, and hide that icon.
fn refresh_icons(data: &Path) {
    let hicolor = hicolor_in(data);
    let _ = fs::File::open(&hicolor).and_then(|dir| dir.set_modified(std::time::SystemTime::now()));
    if hicolor.join("icon-theme.cache").is_file() {
        if let Some(tool) = ["gtk-update-icon-cache", "gtk4-update-icon-cache"].into_iter().find_map(paths::find_in_path) {
            let _ = proc::run(&tool, &[OsStr::new("--quiet"), OsStr::new("--ignore-theme-index"), hicolor.as_os_str()]);
        }
    }
}

/// Puts the control panel, as it is installed next to this program, in the desktop's app menu: the
/// icons, then the desktop file. Returns the desktop file.
pub fn install() -> Result<PathBuf, String> {
    if !cfg!(target_os = "linux") {
        return Err("The app menu entry is for Linux.".to_string());
    }
    let panel = paths::panel_exe();
    if !panel.is_file() {
        return Err(format!("crafthub-panel was not found next to this program ({}).", panel.display()));
    }
    let entry = desktop_entry(&panel)?;
    let data = paths::local_data_dir();
    for (size, picture) in ICON_SIZES.iter().zip(ICON_PICTURES) {
        write(&icon_file_in(&data, size), picture)?;
    }
    refresh_icons(&data);
    // The desktop notices a new desktop file by itself. Its MIME cache (update-desktop-database) is
    // not concerned: the entry opens no kind of file.
    let desktop = desktop_file_in(&data);
    write(&desktop, entry.as_bytes())?;
    Ok(desktop)
}

/// The files `install` puts in place that are there: the desktop file, then the icons.
pub fn installed_files() -> Vec<PathBuf> {
    if !cfg!(target_os = "linux") {
        return Vec::new();
    }
    let data = paths::local_data_dir();
    std::iter::once(desktop_file_in(&data))
        .chain(ICON_SIZES.iter().map(|size| icon_file_in(&data, size)))
        .filter(|file| fs::symlink_metadata(file).is_ok())
        .collect()
}

/// Takes the entry out of the app menu, with its icons. It carries on past a file that can't be
/// deleted, and says which (the last one, if there are several).
pub fn uninstall() -> Result<(), String> {
    let mut outcome = Ok(());
    for file in installed_files() {
        if let Err(e) = fs::remove_file(&file) {
            outcome = Err(format!("could not delete {}: {e}", file.display()));
        }
    }
    refresh_icons(&paths::local_data_dir());
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exec_line(entry: &str) -> &str {
        entry.lines().find_map(|line| line.strip_prefix("Exec=")).unwrap()
    }

    #[test]
    fn the_entry_opens_the_panel_with_its_name_and_icon() {
        let entry = desktop_entry(Path::new("/home/ada/.local/bin/crafthub-panel")).unwrap();
        assert!(entry.starts_with("[Desktop Entry]\n"));
        for line in [
            "Type=Application",
            "Name=Craft MCP Hub",
            "TryExec=/home/ada/.local/bin/crafthub-panel",
            "Exec=\"/home/ada/.local/bin/crafthub-panel\"",
            "Icon=craft-mcp-hub",
            "Terminal=false",
            "Categories=Development;",
            "Keywords=crafthub;MCP;ArtCraft;",
            "StartupWMClass=craft-mcp-hub",
        ] {
            assert!(entry.lines().any(|l| l == line), "{line} is missing from:\n{entry}");
        }
        assert!(entry.ends_with('\n'));
    }

    #[test]
    fn the_panel_path_is_quoted_as_the_specification_asks() {
        let exec = |path: &str| exec_line(&desktop_entry(Path::new(path)).unwrap()).to_string();
        assert_eq!(exec("/home/ada/My Apps/crafthub-panel"), r#""/home/ada/My Apps/crafthub-panel""#);
        // In the file: `\"` as `\\"`, `$` as `\\$`, a backslash as four (the specification's examples).
        assert_eq!(exec(r#"/home/ada/"quoted"/crafthub-panel"#), r#""/home/ada/\\"quoted\\"/crafthub-panel""#);
        assert_eq!(exec("/home/ada/$HOME/crafthub-panel"), r#""/home/ada/\\$HOME/crafthub-panel""#);
        assert_eq!(exec(r"/home/ada/back\slash/crafthub-panel"), r#""/home/ada/back\\\\slash/crafthub-panel""#);
        assert_eq!(exec("/home/ada/`tick`/crafthub-panel"), r#""/home/ada/\\`tick\\`/crafthub-panel""#);
        assert_eq!(exec("/home/ada/100%/crafthub-panel"), r#""/home/ada/100%%/crafthub-panel""#);
        assert_eq!(exec("/home/ada/semi;colon/crafthub-panel"), r#""/home/ada/semi;colon/crafthub-panel""#);
    }

    #[test]
    fn a_line_break_in_the_path_stays_on_one_line() {
        let entry = desktop_entry(Path::new("/home/ada/two\nlines/crafthub-panel")).unwrap();
        assert_eq!(exec_line(&entry), r#""/home/ada/two\nlines/crafthub-panel""#);
        assert!(entry.lines().any(|l| l == r"TryExec=/home/ada/two\nlines/crafthub-panel"));
    }

    /// Reads `Exec` back as a desktop would: the value unescaped, `%%` made `%`, then the quoted
    /// argument unquoted. It must give the path back.
    fn read_back(exec: &str) -> String {
        let mut value = String::new();
        let mut chars = exec.chars();
        while let Some(c) = chars.next() {
            if c != '\\' {
                value.push(c);
                continue;
            }
            match chars.next().unwrap() {
                'n' => value.push('\n'),
                't' => value.push('\t'),
                'r' => value.push('\r'),
                's' => value.push(' '),
                '\\' => value.push('\\'),
                other => panic!("\\{other} is not a valid escape"),
            }
        }
        let value = value.replace("%%", "%");
        let inner = value.strip_prefix('"').and_then(|v| v.strip_suffix('"')).expect("quoted");
        let mut argument = String::new();
        let mut chars = inner.chars();
        while let Some(c) = chars.next() {
            if c == '\\' {
                argument.push(chars.next().unwrap());
            } else {
                assert!(c != '"' && c != '`' && c != '$', "{c} is not escaped in {exec}");
                argument.push(c);
            }
        }
        argument
    }

    #[test]
    fn every_path_reads_back_the_same() {
        for path in ["/plain/crafthub-panel", "/a b/\"c\"/$d/%e/\\f/`g`/'h'/;i/#j/crafthub-panel", "/tab\there/new\nline/crafthub-panel"] {
            assert_eq!(read_back(&exec_argument(path)), path);
        }
    }

    #[test]
    fn the_files_go_where_the_desktop_looks() {
        let data = Path::new("/home/ada/.local/share");
        assert_eq!(desktop_file_in(data), Path::new("/home/ada/.local/share/applications/craft-mcp-hub.desktop"));
        assert_eq!(icon_file_in(data, "48x48"), Path::new("/home/ada/.local/share/icons/hicolor/48x48/apps/craft-mcp-hub.png"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_icons_are_pictures_of_their_size() {
        assert_eq!(ICON_PICTURES.len(), ICON_SIZES.len());
        for (size, picture) in ICON_SIZES.iter().zip(ICON_PICTURES) {
            assert_eq!(&picture[1..4], b"PNG", "{size}");
            // The PNG header gives the width and height, big-endian, at bytes 16 and 20.
            let side = |at: usize| u32::from_be_bytes(picture[at..at + 4].try_into().unwrap());
            assert_eq!(format!("{}x{}", side(16), side(20)), *size);
        }
    }
}
