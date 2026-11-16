//! Writing files that other programs also read, and that may hold their secrets.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

/// Replaces a file's content in one step: readers see the old file or the new one, never half.
///
/// The new file is private to the user while it is written and then takes over the old file's
/// permissions, so a settings file holding API keys is never readable by more people than before.
/// A symbolic link is written through, not replaced: settings kept in a dotfiles folder stay linked.
pub fn write_atomic(path: &Path, content: &[u8]) -> io::Result<()> {
    let target = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    if let Some(dir) = target.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut name = target.file_name().unwrap_or_default().to_os_string();
    name.push(".crafthub-tmp");
    let temp = target.with_file_name(name);
    let written = write_private(&temp, content).and_then(|()| {
        if let Ok(existing) = fs::metadata(&target) {
            fs::set_permissions(&temp, existing.permissions())?;
        }
        fs::rename(&temp, &target)
    });
    if written.is_err() {
        let _ = fs::remove_file(&temp);
    }
    written
}

fn write_private(file: &Path, content: &[u8]) -> io::Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(file)?;
    file.write_all(content)?;
    file.sync_all()
}

/// Creates a folder (and its parents) that only the user may enter.
pub fn create_private_dir(dir: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_content_and_leaves_no_temporary_file() {
        let dir = std::env::temp_dir().join(format!("crafthub-files-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("settings.json");
        write_atomic(&file, b"one").unwrap();
        write_atomic(&file, b"two").unwrap();
        assert_eq!(fs::read_to_string(&file).unwrap(), "two");
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn keeps_a_private_file_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("crafthub-files-mode-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("settings.json");
        write_atomic(&file, b"one").unwrap();
        assert_eq!(fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
        fs::set_permissions(&file, fs::Permissions::from_mode(0o640)).unwrap();
        write_atomic(&file, b"two").unwrap();
        assert_eq!(fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o640);
        fs::remove_dir_all(&dir).unwrap();
    }
}
