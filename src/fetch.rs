//! Fetching an app's command-line tool where the ArtCraft launcher doesn't install it.
//!
//! On macOS the launcher installs only the desktop app (`/Applications/<Name>.app`). The app's
//! command-line tool, which runs its MCP engine, is published next to it in the same release as
//! `<binary>-cli-<version>-macos-universal.zip`. The hub downloads the one that matches the
//! installed app's version into its tools folder, and installs it only when the download matches the
//! release's SHA256SUMS.txt and the program is signed by the app's own Apple team (from the catalog).

use crate::catalog::CatalogApp;
use crate::{discover, files, paths, proc};
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// A download lock older than this was left by a download that died: curl gives up after 15 minutes.
const LOCK_GIVEN_UP_AFTER: Duration = Duration::from_secs(20 * 60);

/// The file in a fetched tool's folder that says which version it is.
const VERSION_FILE: &str = "VERSION";

fn binary(app: &CatalogApp) -> &str {
    if app.binary.is_empty() {
        &app.id
    } else {
        &app.binary
    }
}

/// A version as it may appear in a URL and a file name: nothing but digits, letters, `.`, `-`, `+`.
fn plain_version(version: &str) -> bool {
    !version.is_empty() && version.len() < 40 && version.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+'))
}

/// The version of the desktop app installed on this computer.
pub fn desktop_version(app: &CatalogApp) -> Option<String> {
    let plist = discover::mac_bundle(app)?.join("Contents/Info.plist");
    let out = proc::run(
        Path::new("/usr/bin/plutil"),
        &[OsStr::new("-extract"), OsStr::new("CFBundleShortVersionString"), OsStr::new("raw"), OsStr::new("-o"), OsStr::new("-"), plist.as_os_str()],
    );
    let version = out.stdout.trim().to_string();
    (out.ok && plain_version(&version)).then_some(version)
}

/// The version of the tool the hub fetched for the app, if it fetched one.
pub fn fetched_version(app: &CatalogApp) -> Option<String> {
    fs::read_to_string(discover::tool_dir(app).join(VERSION_FILE)).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

/// Whether the tool the hub fetched is for another version than the desktop app now installed (the
/// launcher updated the app since).
pub fn outdated(app: &CatalogApp) -> bool {
    match (fetched_version(app), desktop_version(app)) {
        (Some(fetched), Some(desktop)) => fetched != desktop,
        _ => false,
    }
}

/// The checksum SHA256SUMS.txt gives for `asset`.
fn listed_sum(sums: &str, asset: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        let sum = parts.next()?;
        let name = parts.next()?.trim_start_matches('*');
        (name == asset && sum.len() == 64 && sum.chars().all(|c| c.is_ascii_hexdigit())).then(|| sum.to_ascii_lowercase())
    })
}

/// The file called `name` in `dir` or one folder below it.
fn find_file(dir: &Path, name: &str) -> Option<PathBuf> {
    let direct = dir.join(name);
    if direct.is_file() {
        return Some(direct);
    }
    fs::read_dir(dir).ok()?.flatten().map(|entry| entry.path()).filter(|p| p.is_dir()).map(|sub| sub.join(name)).find(|file| file.is_file())
}

fn tool(name: &str) -> PathBuf {
    paths::find_in_path(name).unwrap_or_else(|| PathBuf::from("/usr/bin").join(name))
}

fn download(url: &str, to: &Path) -> Result<(), String> {
    let out = proc::run(
        &tool("curl"),
        &[
            OsStr::new("-fsSL"),
            OsStr::new("--proto"),
            OsStr::new("=https"),
            OsStr::new("--max-time"),
            OsStr::new("900"),
            OsStr::new("-o"),
            to.as_os_str(),
            OsStr::new(url),
        ],
    );
    if out.ok {
        Ok(())
    } else {
        Err(format!("could not download {url}: {}", out.text))
    }
}

/// Downloads the app's command-line tool for the installed version of its desktop app and puts it
/// in the hub's tools folder. Returns the program.
pub fn fetch_cli(app: &CatalogApp) -> Result<PathBuf, String> {
    if !cfg!(target_os = "macos") {
        return Err(format!("{}'s command-line tool comes with its installer on this system.", app.name));
    }
    if discover::mac_bundle(app).is_none() {
        return Err(format!("{} is not installed: install it with the ArtCraft launcher first.", app.name));
    }
    if app.repo.is_empty() {
        return Err(format!("The catalog doesn't say where {} is published, so its command-line tool can't be fetched.", app.name));
    }
    let version = desktop_version(app).ok_or_else(|| format!("Could not read which version of {} is installed.", app.name))?;
    let program = format!("{}-cli", binary(app));
    let asset = format!("{program}-{version}-macos-universal.zip");
    let base = format!("https://github.com/{}/releases/download/v{version}", app.repo);

    let tools = paths::tools_dir();
    files::create_private_dir(&paths::hub_home()).map_err(|e| e.to_string())?;
    fs::create_dir_all(&tools).map_err(|e| format!("could not create {}: {e}", tools.display()))?;
    // One download per app at a time: the panel, a command and a server may all want it at once.
    let lock = tools.join(format!(".{}.lock", app.id));
    let abandoned = fs::metadata(&lock).and_then(|m| m.modified()).ok().and_then(|at| at.elapsed().ok()).is_some_and(|age| age > LOCK_GIVEN_UP_AFTER);
    if abandoned {
        let _ = fs::remove_file(&lock);
    }
    if fs::OpenOptions::new().write(true).create_new(true).open(&lock).is_err() {
        return Err(format!("{}'s command-line tool is already being downloaded.", app.name));
    }
    let staging = tools.join(format!(".{}-{}", app.id, std::process::id()));
    let _ = fs::remove_dir_all(&staging);
    let outcome = fs::create_dir_all(&staging).map_err(|e| e.to_string()).and_then(|()| fetch_into(app, &staging, &base, &asset, &program, &version));
    let _ = fs::remove_dir_all(&staging);
    let _ = fs::remove_file(&lock);
    let program = outcome?;
    discover::forget(&app.id);
    Ok(program)
}

fn fetch_into(app: &CatalogApp, staging: &Path, base: &str, asset: &str, program: &str, version: &str) -> Result<PathBuf, String> {
    let sums_file = staging.join("SHA256SUMS.txt");
    download(&format!("{base}/SHA256SUMS.txt"), &sums_file)?;
    let sums = fs::read_to_string(&sums_file).map_err(|e| e.to_string())?;
    let expected = listed_sum(&sums, asset).ok_or_else(|| format!("The {} {version} release has no {asset}.", app.name))?;

    let zip = staging.join(asset);
    download(&format!("{base}/{asset}"), &zip)?;
    let summed = proc::run(&tool("shasum"), &[OsStr::new("-a"), OsStr::new("256"), zip.as_os_str()]);
    let actual = summed.stdout.split_whitespace().next().unwrap_or("").to_ascii_lowercase();
    if !summed.ok || actual != expected {
        return Err(format!("The download of {asset} does not match its checksum; nothing was installed."));
    }

    let unpacked = staging.join("unpacked");
    let out = proc::run(Path::new("/usr/bin/ditto"), &[OsStr::new("-x"), OsStr::new("-k"), zip.as_os_str(), unpacked.as_os_str()]);
    if !out.ok {
        return Err(format!("could not unpack {asset}: {}", out.text));
    }
    let file = find_file(&unpacked, program).ok_or_else(|| format!("{asset} has no {program} in it."))?;
    if !app.mac_team.is_empty() {
        let verified = proc::run(Path::new("/usr/bin/codesign"), &[OsStr::new("--verify"), OsStr::new("--strict"), file.as_os_str()]);
        let signer = proc::run(Path::new("/usr/bin/codesign"), &[OsStr::new("-dv"), file.as_os_str()]);
        let team = format!("TeamIdentifier={}", app.mac_team);
        if !verified.ok || !signer.text.lines().any(|line| line.trim() == team) {
            return Err(format!("{program} in {asset} is not signed by {}'s developer; nothing was installed.", app.name));
        }
    }
    let folder = file.parent().ok_or("unexpected archive layout")?.to_path_buf();
    fs::write(folder.join(VERSION_FILE), version).map_err(|e| e.to_string())?;

    // Swapped in whole: a server still running the old tool keeps it until it restarts.
    let target = discover::tool_dir(app);
    let old = staging.join("previous");
    if target.exists() {
        fs::rename(&target, &old).map_err(|e| format!("could not replace {}: {e}", target.display()))?;
    }
    if let Err(e) = fs::rename(&folder, &target) {
        let _ = fs::rename(&old, &target);
        return Err(format!("could not install into {}: {e}", target.display()));
    }
    Ok(target.join(program))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksums_are_read_by_file_name() {
        let sums = "1adc9bf3d319e707e9247004db05a32a059634e3710fe79bc14f92f689ca4d4a  pdfcraft-0.21.1-macos-universal.dmg\n\
                    337A9F644744B57E10DE41C57366E53C5CCF8840A4D5DC7322B17B9B83FDC2FA *pdfcraft-cli-0.21.1-macos-universal.zip\n";
        assert_eq!(
            listed_sum(sums, "pdfcraft-cli-0.21.1-macos-universal.zip").as_deref(),
            Some("337a9f644744b57e10de41c57366e53c5ccf8840a4d5dc7322b17b9b83fdc2fa")
        );
        assert_eq!(listed_sum(sums, "pdfcraft-cli-0.21.1-macos"), None);
        assert_eq!(listed_sum("short  x.zip", "x.zip"), None);
    }

    #[test]
    fn versions_must_be_plain() {
        assert!(plain_version("0.21.1"));
        assert!(plain_version("1.0.0-beta.2"));
        assert!(!plain_version("0.1/../../x"));
        assert!(!plain_version("0.1?x=1"));
        assert!(!plain_version(""));
    }
}
