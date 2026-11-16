//! The ArtCraft app catalog: which apps exist and what their programs are called on each system.
//!
//! It comes from the ArtCraft launcher's own catalog, so the hub knows a new app as soon as the
//! launcher does: the copy the launcher keeps on this computer, else the one `crafthub catalog
//! --update` downloaded, else the snapshot built into this program.

use crate::{paths, proc};
use serde::Deserialize;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

pub const CATALOG_URL: &str = "https://getartcraft.com/launcher-catalog.toml";
const SNAPSHOT: &str = include_str!("../assets/catalog.toml");

#[derive(Debug, Clone, Default, Deserialize)]
pub struct CatalogApp {
    pub id: String,
    #[serde(default)]
    pub name: String,
    /// The desktop app's program name; the command-line tool is `<binary>-cli`.
    #[serde(default)]
    pub binary: String,
    #[serde(default)]
    pub tagline: String,
    /// `official` or `community`.
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub publisher: String,
    #[serde(default)]
    pub mac_bundle_name: String,
    #[serde(default)]
    pub flatpak_id: String,
    #[serde(default)]
    pub color: Option<[u8; 3]>,
    #[serde(default)]
    pub former_ids: Vec<String>,
    #[serde(default)]
    pub former_names: Vec<String>,
}

#[derive(Deserialize)]
struct CatalogFile {
    #[serde(default)]
    apps: Vec<CatalogApp>,
}

pub struct Catalog {
    pub apps: Vec<CatalogApp>,
    /// Where this copy came from, for `crafthub catalog`.
    pub source: String,
}

impl Catalog {
    pub fn get(&self, id: &str) -> Option<&CatalogApp> {
        self.apps.iter().find(|a| a.id == id)
    }
}

fn parse(toml_text: &str) -> Option<Vec<CatalogApp>> {
    let file: CatalogFile = toml::from_str(toml_text).ok()?;
    let apps: Vec<CatalogApp> = file.apps.into_iter().filter(|a| !a.id.is_empty()).collect();
    (!apps.is_empty()).then_some(apps)
}

/// Where the launcher may keep its copy of the catalog (a JSON file holding the TOML text).
fn launcher_copies() -> Vec<PathBuf> {
    let mut files = Vec::new();
    for base in [paths::local_data_dir(), paths::cache_dir(), paths::app_config_dir()] {
        for folder in ["ArtCraft Launcher", "artcraft-launcher", "ai.storyteller.artcraft-launcher"] {
            let file = base.join(folder).join("launcher-catalog.json");
            if !files.contains(&file) {
                files.push(file);
            }
        }
    }
    files
}

/// The apps in the launcher's copy: a JSON file holding the catalog's TOML text.
fn from_launcher_copy(json: &str) -> Option<Vec<CatalogApp>> {
    serde_json::from_str::<Value>(json).ok()?.get("catalog").and_then(Value::as_str).and_then(parse)
}

/// The apps in a catalog file. The panel loads the catalog every couple of seconds, so the file is
/// read and parsed again only when it has changed.
fn read(file: &Path, extract: fn(&str) -> Option<Vec<CatalogApp>>) -> Option<Vec<CatalogApp>> {
    type Cached = (PathBuf, paths::FileStamp, Vec<CatalogApp>);
    static CACHE: Mutex<Option<Cached>> = Mutex::new(None);

    let stamp = paths::file_stamp(file)?;
    let mut cache = CACHE.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some((cached_file, cached_stamp, apps)) = cache.as_ref() {
        if cached_file == file && *cached_stamp == stamp {
            return Some(apps.clone());
        }
    }
    let apps = extract(&fs::read_to_string(file).ok()?)?;
    *cache = Some((file.to_path_buf(), stamp, apps.clone()));
    Some(apps)
}

pub fn load() -> Catalog {
    for file in launcher_copies() {
        if let Some(apps) = read(&file, from_launcher_copy) {
            return Catalog { apps, source: format!("the ArtCraft launcher ({})", file.display()) };
        }
    }
    let downloaded = paths::catalog_cache();
    if let Some(apps) = read(&downloaded, parse) {
        return Catalog { apps, source: format!("downloaded copy ({})", downloaded.display()) };
    }
    static BUILT_IN: OnceLock<Vec<CatalogApp>> = OnceLock::new();
    Catalog { apps: BUILT_IN.get_or_init(|| parse(SNAPSHOT).unwrap_or_default()).clone(), source: "built-in snapshot".to_string() }
}

/// Downloads the current catalog with the system's `curl` and keeps it. Returns how many apps it lists.
pub fn update() -> Result<usize, String> {
    let curl = paths::find_in_path("curl").ok_or("curl was not found on this computer")?;
    let out = proc::run(&curl, &["-fsSL", "--max-time", "30", CATALOG_URL]);
    if !out.ok {
        return Err(format!("could not download {CATALOG_URL}: {}", out.text));
    }
    let apps = parse(&out.stdout).ok_or("the downloaded catalog could not be read")?;
    fs::create_dir_all(paths::hub_home()).map_err(|e| e.to_string())?;
    fs::write(paths::catalog_cache(), &out.stdout).map_err(|e| e.to_string())?;
    Ok(apps.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_lists_the_suite() {
        let apps = parse(SNAPSHOT).expect("the built-in snapshot parses");
        let photocraft = apps.iter().find(|a| a.id == "photocraft").expect("photocraft is listed");
        assert_eq!(photocraft.binary, "photocraft");
        assert_eq!(photocraft.name, "PhotoCraft");
        assert!(photocraft.color.is_some());
        assert!(apps.iter().any(|a| a.id == "pdfcraft" && a.former_ids == ["printcraft"]));
    }
}
