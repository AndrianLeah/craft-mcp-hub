//! The ArtCraft app catalog: which apps exist and what their programs are called on each system.
//!
//! It comes from the ArtCraft launcher's own catalog, so the hub knows a new app as soon as the
//! launcher does: the copy the launcher keeps on this computer, else the one `crafthub catalog
//! --update` downloaded, else the snapshot built into this program. The apps the hub lists although
//! the launcher doesn't have them yet (`assets/extra-apps.toml`) are added after it.
//!
//! ArtCraft signs the catalog, and a copy is used only when its signature checks out against the
//! key the launcher pins, checked as the launcher checks it. A copy that fails, or has none, is
//! passed over for the next one. The snapshot ships with the hub and the extra apps are the hub's
//! own, so neither has a signature.

use crate::{files, paths, proc};
use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use ed25519_dalek::{Signature, VerifyingKey};
use serde::Deserialize;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

pub const CATALOG_URL: &str = "https://getartcraft.com/launcher-catalog.toml";
pub const SIGNATURE_URL: &str = "https://getartcraft.com/launcher-catalog.toml.sig";
const SNAPSHOT: &str = include_str!("../assets/catalog.toml");
const EXTRA: &str = include_str!("../assets/extra-apps.toml");

/// The public key the launcher pins, the same file as its
/// `crates/core/assets/launcher-catalog-public-key.pem` (storytold/craft-launcher). There is one
/// key: a new one takes a new launcher, and a new hub.
const PUBLIC_KEY_PEM: &str = include_str!("../assets/launcher-catalog-public-key.pem");
/// The launcher's limits. A larger catalog or signature is refused before it is checked.
const MAX_CATALOG_BYTES: usize = 1 << 20;
const MAX_SIGNATURE_BYTES: usize = 128;
const NO_SIGNATURE: &str = "it has no signature";

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
    /// The app's GitHub repository (`owner/name`), whose releases carry its command-line tool.
    #[serde(default)]
    pub repo: String,
    /// The Apple team that signs the app's macOS programs.
    #[serde(default)]
    pub mac_team: String,
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
    /// Where this copy came from, for `crafthub catalog` and `crafthub doctor`.
    pub source: String,
    /// Whether it is a copy whose signature checked out, rather than the built-in snapshot.
    pub signed: bool,
    /// The copies passed over for it, each a sentence that says why.
    pub skipped: Vec<String>,
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

/// The launcher's key. The PEM file holds it as a SubjectPublicKeyInfo: a fixed header that says
/// "Ed25519 key", then the key's 32 bytes.
fn pinned_key() -> &'static VerifyingKey {
    const ED25519_HEADER: [u8; 12] = [0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00];
    static KEY: OnceLock<VerifyingKey> = OnceLock::new();
    KEY.get_or_init(|| {
        let base64: String = PUBLIC_KEY_PEM.lines().filter(|line| !line.starts_with("-----")).collect();
        let der = STANDARD.decode(base64.trim()).unwrap_or_default();
        let key = der.strip_prefix(&ED25519_HEADER[..]).and_then(|key| <[u8; 32]>::try_from(key).ok());
        key.and_then(|key| VerifyingKey::from_bytes(&key).ok()).expect("the launcher's key, built in, is an Ed25519 public key")
    })
}

/// What a copy of the catalog gave: its apps, or why it can't be used.
type Checked = Result<Vec<CatalogApp>, String>;

/// The apps in a signed catalog, if its signature checks out. As the launcher checks it: the
/// signature is an Ed25519 signature over the catalog's exact bytes, in base64 with any whitespace
/// around it, and it must pass the strict check (no weak keys, no malleable signatures).
fn verified(toml_text: &str, signature: &str, key: &VerifyingKey) -> Checked {
    if toml_text.len() > MAX_CATALOG_BYTES {
        return Err("it is larger than the 1 MiB a catalog may be".into());
    }
    if signature.trim().is_empty() {
        return Err(NO_SIGNATURE.into());
    }
    let signature = Some(signature)
        .filter(|signature| signature.len() <= MAX_SIGNATURE_BYTES)
        .and_then(|signature| STANDARD.decode(signature.trim()).ok())
        .and_then(|bytes| Signature::from_slice(&bytes).ok())
        .ok_or("its signature is not an Ed25519 signature in base64")?;
    key.verify_strict(toml_text.as_bytes(), &signature).map_err(|_| "its signature doesn't match the ArtCraft launcher's key")?;
    parse(toml_text).ok_or_else(|| "it lists no app the hub can read".into())
}

/// A file's text. One larger than `max` bytes is refused unread.
fn read_text(file: &Path, max: usize) -> Result<String, String> {
    let size = fs::metadata(file).map_err(|e| format!("it can't be read: {e}"))?.len();
    if size > max as u64 {
        return Err(format!("it is larger than the {max} bytes it may be"));
    }
    fs::read_to_string(file).map_err(|e| format!("it can't be read: {e}"))
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

/// The apps in the launcher's copy: a JSON file holding the catalog's TOML text, `catalog`, and
/// the signature the launcher downloaded with it, `signature`.
fn from_launcher_copy(file: &Path, key: &VerifyingKey) -> Checked {
    // JSON escaping can turn one byte of the catalog into six.
    let json = read_text(file, MAX_CATALOG_BYTES * 6 + MAX_SIGNATURE_BYTES * 6 + 1024)?;
    let saved: Value = serde_json::from_str(&json).map_err(|_| "it is not the launcher's catalog file")?;
    let catalog = saved.get("catalog").and_then(Value::as_str).ok_or("it holds no catalog")?;
    verified(catalog, saved.get("signature").and_then(Value::as_str).unwrap_or_default(), key)
}

/// The downloaded copy's signature, kept next to it.
fn signature_file(catalog: &Path) -> PathBuf {
    let mut name = catalog.file_name().unwrap_or_default().to_os_string();
    name.push(".sig");
    catalog.with_file_name(name)
}

/// The apps in the copy `crafthub catalog --update` downloaded: the catalog as it was served,
/// and its signature in the file next to it.
fn from_download(file: &Path, key: &VerifyingKey) -> Checked {
    let catalog = read_text(file, MAX_CATALOG_BYTES)?;
    let signature = signature_file(file);
    if !signature.exists() {
        return Err(NO_SIGNATURE.into());
    }
    verified(&catalog, &read_text(&signature, MAX_SIGNATURE_BYTES)?, key)
}

/// A copy of the catalog as it was last checked.
struct Seen {
    file: PathBuf,
    /// When the copy and its signature file last changed, and their sizes.
    stamps: (paths::FileStamp, Option<paths::FileStamp>),
    key: [u8; 32],
    outcome: Checked,
}

/// Checks the copy of the catalog in `file`, with its signature in `signature` when it is kept
/// apart. `None` when there is no such copy. The panel loads the catalog every couple of seconds,
/// so a copy is read and checked again only when one of its files has changed.
fn checked(file: &Path, signature: Option<&Path>, key: &VerifyingKey, check: impl FnOnce() -> Checked) -> Option<Checked> {
    static SEEN: Mutex<Vec<Seen>> = Mutex::new(Vec::new());

    let stamps = (paths::file_stamp(file)?, signature.and_then(paths::file_stamp));
    let key = key.to_bytes();
    let mut seen = SEEN.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(copy) = seen.iter().find(|copy| copy.file == file && copy.stamps == stamps && copy.key == key) {
        return Some(copy.outcome.clone());
    }
    let outcome = check();
    seen.retain(|copy| copy.file != file);
    seen.push(Seen { file: file.to_path_buf(), stamps, key, outcome: outcome.clone() });
    Some(outcome)
}

fn built_in() -> &'static [CatalogApp] {
    static BUILT_IN: OnceLock<Vec<CatalogApp>> = OnceLock::new();
    BUILT_IN.get_or_init(|| parse(SNAPSHOT).unwrap_or_default())
}

pub fn load() -> Catalog {
    with_extra_apps(load_signed(&launcher_copies(), &paths::catalog_cache(), pinned_key()))
}

/// The first copy of the launcher's catalog whose signature checks out against `key`, else the
/// built-in snapshot; with the copies passed over and why. Copies that aren't there aren't mentioned.
fn load_signed(launcher_copies: &[PathBuf], downloaded: &Path, key: &VerifyingKey) -> Catalog {
    let mut skipped = Vec::new();
    for file in launcher_copies {
        match checked(file, None, key, || from_launcher_copy(file, key)) {
            Some(Ok(apps)) => {
                let source = format!("the ArtCraft launcher's copy ({}), signature verified", file.display());
                return Catalog { apps, source, signed: true, skipped };
            }
            Some(Err(why)) => skipped.push(format!("The ArtCraft launcher's copy ({}) was not used: {why}.", file.display())),
            None => {}
        }
    }
    match checked(downloaded, Some(&signature_file(downloaded)), key, || from_download(downloaded, key)) {
        Some(Ok(apps)) => {
            let source = format!("the copy downloaded from getartcraft.com ({}), signature verified", downloaded.display());
            return Catalog { apps, source, signed: true, skipped };
        }
        Some(Err(why)) => skipped.push(format!("The downloaded copy ({}) was not used: {why}.", downloaded.display())),
        None => {}
    }
    Catalog { apps: built_in().to_vec(), source: "the snapshot built into the hub".to_string(), signed: false, skipped }
}

/// The apps the hub lists although the launcher's catalog doesn't have them yet.
fn extra_apps() -> &'static [CatalogApp] {
    static EXTRA_APPS: OnceLock<Vec<CatalogApp>> = OnceLock::new();
    EXTRA_APPS.get_or_init(|| parse(EXTRA).unwrap_or_default())
}

/// Adds the extra apps the catalog doesn't have. One it has, under its id or a former id, is the
/// catalog's: the extra entry steps aside once its app joins the launcher.
fn with_extra_apps(mut catalog: Catalog) -> Catalog {
    let mut added = 0;
    for extra in extra_apps() {
        let known = catalog.apps.iter().any(|app| app.id == extra.id || app.former_ids.contains(&extra.id));
        if !known {
            catalog.apps.push(extra.clone());
            added += 1;
        }
    }
    if added > 0 {
        catalog.source = format!("{}, and {added} app(s) the launcher doesn't list yet", catalog.source);
    }
    catalog
}

/// Downloads a file with the system's `curl`, as text.
fn download(curl: &Path, url: &str, max: usize) -> Result<String, String> {
    let out = proc::run(curl, &["-fsSL", "--max-time", "30", "--max-filesize", &max.to_string(), url]);
    if !out.ok {
        return Err(format!("could not download {url}: {}", out.text));
    }
    Ok(out.stdout)
}

/// Downloads the current catalog and its signature, and keeps them once the signature checks out.
/// Returns how many apps it lists.
pub fn update() -> Result<usize, String> {
    let curl = paths::find_in_path("curl").ok_or("curl was not found on this computer")?;
    let catalog = download(&curl, CATALOG_URL, MAX_CATALOG_BYTES)?;
    let signature = download(&curl, SIGNATURE_URL, MAX_SIGNATURE_BYTES)?;
    let apps = verified(&catalog, &signature, pinned_key()).map_err(|why| format!("The catalog from {CATALOG_URL} was not kept: {why}."))?;
    let file = paths::catalog_cache();
    files::write_atomic(&signature_file(&file), signature.as_bytes()).map_err(|e| e.to_string())?;
    files::write_atomic(&file, catalog.as_bytes()).map_err(|e| e.to_string())?;
    Ok(apps.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    /// A key for the tests: the launcher's private key is ArtCraft's alone.
    fn test_key() -> SigningKey {
        SigningKey::from_bytes(&[7; 32])
    }

    /// A signature as the website serves it: base64, and a line end.
    fn sign(key: &SigningKey, text: &str) -> String {
        format!("{}\n", STANDARD.encode(key.sign(text.as_bytes()).to_bytes()))
    }

    fn launcher_copy(catalog: &str, signature: Option<&str>) -> String {
        let mut saved = serde_json::json!({ "catalog": catalog });
        if let Some(signature) = signature {
            saved["signature"] = signature.into();
        }
        saved.to_string()
    }

    /// An empty folder of the test's own.
    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("crafthub-catalog-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn snapshot_lists_the_suite() {
        let apps = parse(SNAPSHOT).expect("the built-in snapshot parses");
        let photocraft = apps.iter().find(|a| a.id == "photocraft").expect("photocraft is listed");
        assert_eq!(photocraft.binary, "photocraft");
        assert_eq!(photocraft.name, "PhotoCraft");
        assert!(photocraft.color.is_some());
        assert!(apps.iter().any(|a| a.id == "pdfcraft" && a.former_ids == ["printcraft"]));
    }

    #[test]
    fn the_pinned_key_is_the_launchers() {
        // MCowBQYDK2VwAyEASIYrY9GbHPPdZ/URfx25unSh+FHvcQoayxfVvyga4WY= in the launcher's PEM file.
        let hex: String = pinned_key().to_bytes().iter().map(|byte| format!("{byte:02x}")).collect();
        assert_eq!(hex, "48862b63d19b1cf3dd67f5117f1db9ba74a1f851ef710a1acb17d5bf281ae166");
    }

    #[test]
    fn a_catalog_with_a_valid_signature_is_used() {
        let key = test_key();
        let apps = verified(SNAPSHOT, &sign(&key, SNAPSHOT), &key.verifying_key()).expect("it verifies");
        assert!(apps.iter().any(|a| a.id == "photocraft"));

        // The same in the launcher's file, whose signature has the website's line end.
        let dir = folder("valid");
        let file = dir.join("launcher-catalog.json");
        fs::write(&file, launcher_copy(SNAPSHOT, Some(&sign(&key, SNAPSHOT)))).unwrap();
        assert!(from_launcher_copy(&file, &key.verifying_key()).is_ok());
    }

    #[test]
    fn a_tampered_catalog_is_refused() {
        let key = test_key();
        let signature = sign(&key, SNAPSHOT);
        let tampered = SNAPSHOT.replacen("binary = \"photocraft\"", "binary = \"elsewhere\"", 1);
        assert_ne!(tampered, SNAPSHOT);
        let why = verified(&tampered, &signature, &key.verifying_key()).unwrap_err();
        assert!(why.contains("doesn't match"), "{why}");
        // The exact bytes are signed: other line ends are another catalog.
        assert!(verified(&SNAPSHOT.replace('\n', "\r\n"), &signature, &key.verifying_key()).is_err());
        let mut longer = SNAPSHOT.to_string();
        longer.push('\n');
        assert!(verified(&longer, &signature, &key.verifying_key()).is_err());
    }

    #[test]
    fn a_catalog_without_a_signature_is_refused() {
        let key = test_key().verifying_key();
        assert_eq!(verified(SNAPSHOT, "", &key).unwrap_err(), NO_SIGNATURE);
        assert_eq!(verified(SNAPSHOT, " \n", &key).unwrap_err(), NO_SIGNATURE);

        let dir = folder("unsigned");
        let file = dir.join("launcher-catalog.json");
        fs::write(&file, launcher_copy(SNAPSHOT, None)).unwrap();
        assert_eq!(from_launcher_copy(&file, &key).unwrap_err(), NO_SIGNATURE);
        let downloaded = dir.join("catalog.toml");
        fs::write(&downloaded, SNAPSHOT).unwrap();
        assert_eq!(from_download(&downloaded, &key).unwrap_err(), NO_SIGNATURE);
    }

    #[test]
    fn a_catalog_signed_with_another_key_is_refused() {
        let signed_by_test_key = sign(&test_key(), SNAPSHOT);
        let other = SigningKey::from_bytes(&[8; 32]).verifying_key();
        assert!(verified(SNAPSHOT, &signed_by_test_key, &other).unwrap_err().contains("doesn't match"));
        // Above all, the launcher's own key refuses it.
        assert!(verified(SNAPSHOT, &signed_by_test_key, pinned_key()).unwrap_err().contains("doesn't match"));
    }

    #[test]
    fn a_signature_that_is_not_one_is_refused() {
        let key = test_key();
        let short = STANDARD.encode([1u8; 63]);
        for signature in ["not base64!", short.as_str(), &"A".repeat(MAX_SIGNATURE_BYTES + 1)] {
            let why = verified(SNAPSHOT, signature, &key.verifying_key()).unwrap_err();
            assert!(why.contains("not an Ed25519 signature"), "{signature}: {why}");
        }
        let huge = "#".repeat(MAX_CATALOG_BYTES + 1);
        assert!(verified(&huge, &sign(&key, &huge), &key.verifying_key()).unwrap_err().contains("larger"));
    }

    #[test]
    fn a_copy_that_fails_gives_way_to_the_next_one() {
        let key = test_key();
        let dir = folder("order");
        let missing = dir.join("no-launcher").join("launcher-catalog.json");
        let launcher = dir.join("launcher-catalog.json");
        let downloaded = dir.join("catalog.toml");
        let mut theirs = SNAPSHOT.replacen("name = \"PhotoCraft\"", "name = \"PhotoCraft (signed)\"", 1);
        assert_ne!(theirs, SNAPSHOT);

        // The launcher's copy is checked first, and used.
        fs::write(&launcher, launcher_copy(&theirs, Some(&sign(&key, &theirs)))).unwrap();
        let catalog = load_signed(&[missing.clone(), launcher.clone()], &downloaded, &key.verifying_key());
        assert!(catalog.source.contains("launcher's copy") && catalog.source.contains("signature verified"), "{}", catalog.source);
        assert!(catalog.skipped.is_empty(), "a copy that isn't there isn't mentioned: {:?}", catalog.skipped);
        assert_eq!(catalog.get("photocraft").unwrap().name, "PhotoCraft (signed)");

        // Tampered with: the downloaded copy is used instead, and the launcher's is reported.
        let signature = sign(&key, &theirs);
        theirs = theirs.replacen("(signed)", "(changed)", 1);
        let launcher = dir.join("tampered.json");
        fs::write(&launcher, launcher_copy(&theirs, Some(&signature))).unwrap();
        fs::write(&downloaded, SNAPSHOT).unwrap();
        fs::write(signature_file(&downloaded), sign(&key, SNAPSHOT)).unwrap();
        let catalog = load_signed(std::slice::from_ref(&launcher), &downloaded, &key.verifying_key());
        assert!(catalog.source.contains("downloaded") && catalog.signed, "{}", catalog.source);
        assert_eq!(catalog.skipped.len(), 1);
        assert!(catalog.skipped[0].contains("tampered.json") && catalog.skipped[0].contains("doesn't match"), "{:?}", catalog.skipped);
        assert_eq!(catalog.get("photocraft").unwrap().name, "PhotoCraft");

        // The downloaded copy without its signature: the built-in snapshot, with both reasons.
        let downloaded = dir.join("unsigned.toml");
        fs::write(&downloaded, SNAPSHOT).unwrap();
        let catalog = load_signed(std::slice::from_ref(&launcher), &downloaded, &key.verifying_key());
        assert_eq!(catalog.source, "the snapshot built into the hub");
        assert!(!catalog.signed);
        assert_eq!(catalog.skipped.len(), 2);
        assert!(catalog.skipped[1].contains("unsigned.toml") && catalog.skipped[1].contains(NO_SIGNATURE), "{:?}", catalog.skipped);
        assert_eq!(catalog.apps.len(), built_in().len());

        // A signature added later is noticed.
        fs::write(signature_file(&downloaded), sign(&key, SNAPSHOT)).unwrap();
        let catalog = load_signed(std::slice::from_ref(&launcher), &downloaded, &key.verifying_key());
        assert!(catalog.source.contains("unsigned.toml"), "{}", catalog.source);

        // The same files are refused under the launcher's own key.
        let catalog = load_signed(std::slice::from_ref(&launcher), &downloaded, pinned_key());
        assert_eq!(catalog.source, "the snapshot built into the hub");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    #[ignore = "downloads the catalog from getartcraft.com"]
    fn the_published_catalog_verifies_with_the_launchers_key() {
        let curl = paths::find_in_path("curl").expect("curl");
        let catalog = download(&curl, CATALOG_URL, MAX_CATALOG_BYTES).unwrap();
        let signature = download(&curl, SIGNATURE_URL, MAX_SIGNATURE_BYTES).unwrap();
        let apps = verified(&catalog, &signature, pinned_key()).unwrap();
        assert!(apps.iter().any(|a| a.id == "photocraft"));
    }

    #[test]
    fn extra_apps_are_added_after_the_catalog() {
        let shapecraft = extra_apps().iter().find(|a| a.id == "shapecraft").expect("shapecraft is an extra app");
        assert_eq!((shapecraft.name.as_str(), shapecraft.kind.as_str()), ("ShapeCraft", "community"));
        assert_eq!(shapecraft.mac_bundle_name, "ShapeCraft");

        let catalog = with_extra_apps(Catalog { apps: parse(SNAPSHOT).unwrap(), source: "test".into(), signed: false, skipped: Vec::new() });
        let last = catalog.apps.last().unwrap();
        assert_eq!(last.id, "shapecraft", "added after the catalog's own apps");
        assert_eq!(catalog.apps.iter().filter(|a| a.id == "shapecraft").count(), 1);
        assert!(catalog.source.contains("1 app(s) the launcher doesn't list yet"));
    }

    #[test]
    fn the_launchers_entry_wins_over_an_extra_one() {
        let launcher = CatalogApp { id: "shapecraft".into(), name: "ShapeCraft (launcher)".into(), kind: "official".into(), ..Default::default() };
        let catalog = with_extra_apps(Catalog { apps: vec![launcher], source: "test".into(), signed: false, skipped: Vec::new() });
        let shapecraft: Vec<&CatalogApp> = catalog.apps.iter().filter(|a| a.id == "shapecraft").collect();
        assert_eq!(shapecraft.len(), 1);
        assert_eq!(shapecraft[0].name, "ShapeCraft (launcher)");
        assert_eq!(catalog.source, "test", "nothing was added");

        // Also when the launcher lists it under a new id with the old one as a former id.
        let renamed = CatalogApp { id: "shapeforge".into(), former_ids: vec!["shapecraft".into()], ..Default::default() };
        let catalog = with_extra_apps(Catalog { apps: vec![renamed], source: "test".into(), signed: false, skipped: Vec::new() });
        assert!(catalog.apps.iter().all(|a| a.id != "shapecraft"));
    }
}
