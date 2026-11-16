//! Plain-text log files in the hub's `logs` folder.

use crate::{paths, timefmt};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// A log that reaches this size is set aside as `<name>.old.log` and started afresh, so a server
/// that runs for weeks can't fill the disk.
const MAX_SIZE: u64 = 2 * 1024 * 1024;

struct Open {
    file: File,
    size: u64,
}

pub struct Logger {
    path: PathBuf,
    old: PathBuf,
    /// The file stays open between lines; `None` until the first line and after a rotation.
    open: Mutex<Option<Open>>,
}

impl Logger {
    /// The log `<name>.log`.
    pub fn new(name: &str) -> Logger {
        let dir = paths::log_dir();
        let _ = fs::create_dir_all(&dir);
        Logger { path: dir.join(format!("{name}.log")), old: dir.join(format!("{name}.old.log")), open: Mutex::new(None) }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn write(&self, msg: &str) {
        let mut open = self.open.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if open.as_ref().is_some_and(|log| log.size > MAX_SIZE) {
            // Closed before it is renamed: Windows doesn't rename a file that is open.
            *open = None;
            let _ = fs::rename(&self.path, &self.old);
        }
        if open.is_none() {
            let Ok(file) = OpenOptions::new().create(true).append(true).open(&self.path) else { return };
            let size = file.metadata().map(|m| m.len()).unwrap_or(0);
            *open = Some(Open { file, size });
        }
        if let Some(log) = open.as_mut() {
            let line = format!("{}  {}\n", timefmt::stamp(), msg);
            if log.file.write_all(line.as_bytes()).is_ok() {
                log.size += line.len() as u64;
            }
        }
    }
}

/// The log file of an app's server.
pub fn server_log(app: &str) -> PathBuf {
    paths::log_dir().join(format!("{app}.log"))
}
