//! The macOS app, `Craft MCP Hub.app`: the control panel as a real app, in the Applications folder,
//! Launchpad, the Dock and Spotlight, with its own name and icon.
//!
//! The bundle holds both programs: the control panel, named `Craft MCP Hub` there so macOS shows
//! that name everywhere (menu bar, Activity Monitor, "Quit Craft MCP Hub"), and `crafthub` next to
//! it, as the panel expects. `crafthub install-app --link <dir>` also points `<dir>/crafthub` and
//! `<dir>/crafthub-panel` at them, so there is one copy of each and an update replaces both.

use crate::{paths, proc, VERSION};
use std::fs;
use std::path::{Path, PathBuf};

pub const APP_NAME: &str = "Craft MCP Hub";
pub const BUNDLE_ID: &str = "io.github.andrianleah.craft-mcp-hub";
const ICON: &[u8] = include_bytes!("../assets/icon.icns");
/// Each program: its usual name, and its name inside the app.
const PROGRAMS: [(&str, &str); 2] = [("crafthub-panel", APP_NAME), ("crafthub", "crafthub")];

fn bundle_name() -> String {
    format!("{APP_NAME}.app")
}

/// The folders the app may be installed in: /Applications, else ~/Applications.
fn app_dirs() -> [PathBuf; 2] {
    [PathBuf::from("/Applications"), paths::home_dir().join("Applications")]
}

/// Every copy of this app in the Applications folders, told apart from another app of the same name
/// by its bundle id.
pub fn all_installed() -> Vec<PathBuf> {
    app_dirs()
        .into_iter()
        .map(|dir| dir.join(bundle_name()))
        .filter(|app| fs::read_to_string(app.join("Contents/Info.plist")).is_ok_and(|plist| plist.contains(&format!("<string>{BUNDLE_ID}</string>"))))
        .collect()
}

/// The installed app, if there is one.
pub fn installed() -> Option<PathBuf> {
    app_dirs().into_iter().map(|dir| dir.join(bundle_name())).find(|app| app.join("Contents/MacOS").join(APP_NAME).is_file())
}

/// Words Spotlight should also find the app by: its program names, and the name written as one word.
/// Spotlight matches the start of each word of an app's name, so "crafthub" alone wouldn't find
/// "Craft MCP Hub". They go in the app's Spotlight comment (Get Info), which Spotlight searches;
/// keywords in Info.plist are not indexed for apps.
const SEARCH_WORDS: &str = "crafthub crafthub-panel CraftHub ArtCraft MCP";

fn info_plist() -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleDevelopmentRegion</key>
	<string>en</string>
	<key>CFBundleDisplayName</key>
	<string>{APP_NAME}</string>
	<key>CFBundleName</key>
	<string>{APP_NAME}</string>
	<key>CFBundleExecutable</key>
	<string>{APP_NAME}</string>
	<key>CFBundleIconFile</key>
	<string>AppIcon</string>
	<key>CFBundleIdentifier</key>
	<string>{BUNDLE_ID}</string>
	<key>CFBundleInfoDictionaryVersion</key>
	<string>6.0</string>
	<key>CFBundlePackageType</key>
	<string>APPL</string>
	<key>CFBundleShortVersionString</key>
	<string>{VERSION}</string>
	<key>CFBundleVersion</key>
	<string>{VERSION}</string>
	<key>LSApplicationCategoryType</key>
	<string>public.app-category.developer-tools</string>
	<key>LSMinimumSystemVersion</key>
	<string>11.0</string>
	<key>NSHighResolutionCapable</key>
	<true/>
	<key>NSSupportsAutomaticGraphicsSwitching</key>
	<true/>
	<key>NSHumanReadableCopyright</key>
	<string>MIT License. Unofficial; not made by ArtCraft.</string>
</dict>
</plist>
"#
    )
}

fn write_bundle(bundle: &Path, programs: &[(PathBuf, &str)]) -> Result<(), String> {
    let contents = bundle.join("Contents");
    let macos = contents.join("MacOS");
    let resources = contents.join("Resources");
    for dir in [&macos, &resources] {
        fs::create_dir_all(dir).map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    }
    for (source, name) in programs {
        let target = macos.join(name);
        fs::copy(source, &target).map_err(|e| format!("could not copy {} into the app: {e}", source.display()))?;
    }
    fs::write(contents.join("Info.plist"), info_plist()).map_err(|e| e.to_string())?;
    fs::write(contents.join("PkgInfo"), "APPL????").map_err(|e| e.to_string())?;
    fs::write(resources.join("AppIcon.icns"), ICON).map_err(|e| e.to_string())?;
    Ok(())
}

/// Puts the app in /Applications (or ~/Applications when that isn't writable), replacing an older
/// one, and with `link` points the programs in that folder at the app's. Returns the app.
pub fn install(link: Option<&Path>) -> Result<PathBuf, String> {
    if !cfg!(target_os = "macos") {
        return Err("The app bundle is for macOS.".to_string());
    }
    // The programs to put in: the ones this is (resolved, so a link into an older app works too).
    let programs: Vec<(PathBuf, &str)> = PROGRAMS
        .iter()
        .map(|(name, inside)| {
            let exe = if *name == "crafthub" { paths::hub_exe() } else { paths::panel_exe() };
            let exe = fs::canonicalize(&exe).map_err(|_| format!("{} was not found next to this program ({}).", name, exe.display()))?;
            Ok((exe, *inside))
        })
        .collect::<Result<_, String>>()?;

    let mut last_error = String::new();
    for dir in app_dirs() {
        if !dir.is_dir() && fs::create_dir_all(&dir).is_err() {
            continue;
        }
        let staging = dir.join(format!(".{}.new-{}", bundle_name(), std::process::id()));
        let _ = fs::remove_dir_all(&staging);
        if let Err(e) = write_bundle(&staging, &programs) {
            let _ = fs::remove_dir_all(&staging);
            last_error = e;
            continue;
        }
        // Sealed with an ad-hoc signature, so macOS sees one consistent app; it runs without one too.
        let _ = proc::run(Path::new("/usr/bin/codesign"), &["--force", "--deep", "--sign", "-", &staging.to_string_lossy()]);
        let bundle = dir.join(bundle_name());
        let old = dir.join(format!(".{}.old-{}", bundle_name(), std::process::id()));
        if bundle.exists() {
            if let Err(e) = fs::rename(&bundle, &old) {
                let _ = fs::remove_dir_all(&staging);
                last_error = format!("could not replace {}: {e}", bundle.display());
                continue;
            }
        }
        if let Err(e) = fs::rename(&staging, &bundle) {
            let _ = fs::rename(&old, &bundle);
            let _ = fs::remove_dir_all(&staging);
            last_error = format!("could not install {}: {e}", bundle.display());
            continue;
        }
        let _ = fs::remove_dir_all(&old);
        register(&bundle);
        if let Some(link) = link {
            link_programs(&bundle, link)?;
        }
        remove_other_copy(&bundle);
        return Ok(bundle);
    }
    Err(format!("The app could not be installed in /Applications or ~/Applications: {last_error}"))
}

/// Tells Launch Services and Spotlight about the app now, rather than whenever they next look.
fn register(bundle: &Path) {
    let lsregister = Path::new("/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister");
    let bundle = bundle.to_string_lossy();
    // Kept as an extended attribute of the folder, outside what the signature covers.
    let comment = format!(r#"<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><string>{SEARCH_WORDS}</string></plist>"#);
    let _ = proc::run(Path::new("/usr/bin/xattr"), &["-w", "com.apple.metadata:kMDItemFinderComment", &comment, &bundle]);
    if lsregister.is_file() {
        let _ = proc::run(lsregister, &["-f", &bundle]);
    }
    let _ = proc::run(Path::new("/usr/bin/mdimport"), &[&*bundle]);
}

/// An older copy in the other Applications folder would show up twice in Launchpad and Spotlight.
fn remove_other_copy(bundle: &Path) {
    for dir in app_dirs() {
        let other = dir.join(bundle_name());
        if other != bundle && other.join("Contents/MacOS").join(APP_NAME).is_file() {
            let _ = fs::remove_dir_all(other);
        }
    }
}

/// Makes `<dir>/crafthub` and `<dir>/crafthub-panel` links to the app's programs.
#[cfg(not(unix))]
fn link_programs(_bundle: &Path, _dir: &Path) -> Result<(), String> {
    Err("Linking the programs is for macOS.".to_string())
}

/// Makes `<dir>/crafthub` and `<dir>/crafthub-panel` links to the app's programs.
#[cfg(unix)]
fn link_programs(bundle: &Path, dir: &Path) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    for (name, inside) in PROGRAMS {
        let link = dir.join(name);
        let target = bundle.join("Contents/MacOS").join(inside);
        let temporary = dir.join(format!(".{name}.link-{}", std::process::id()));
        let _ = fs::remove_file(&temporary);
        std::os::unix::fs::symlink(&target, &temporary).map_err(|e| format!("could not link {}: {e}", link.display()))?;
        fs::rename(&temporary, &link).map_err(|e| format!("could not replace {}: {e}", link.display()))?;
    }
    Ok(())
}

/// Opens the installed app, as Finder would, when `panel` is its program (so a build run from
/// elsewhere opens itself, not the installed one). `false` when it didn't.
pub fn open(panel: &Path) -> bool {
    let Some(bundle) = installed() else { return false };
    let ours = fs::canonicalize(panel).ok().zip(fs::canonicalize(&bundle).ok()).is_some_and(|(panel, bundle)| panel.starts_with(bundle));
    ours && proc::run(Path::new("/usr/bin/open"), &[&*bundle.to_string_lossy()]).ok
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_app_is_named_for_people() {
        let plist = info_plist();
        assert!(plist.contains("<string>Craft MCP Hub</string>"));
        assert!(plist.contains("<key>CFBundleExecutable</key>\n\t<string>Craft MCP Hub</string>"));
        assert!(plist.contains(&format!("<string>{VERSION}</string>")));
        assert_eq!(&ICON[..4], b"icns");
    }
}
