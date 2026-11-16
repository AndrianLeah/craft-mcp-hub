//! Plain-text log files in the hub's `logs` folder.

use crate::{paths, timefmt};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub struct Logger {
    path: PathBuf,
    gate: Mutex<()>,
}

impl Logger {
    /// Opens `<name>.log`, setting a large one aside as `<name>.old.log` first.
    pub fn new(name: &str) -> Logger {
        let dir = paths::log_dir();
        let _ = fs::create_dir_all(&dir);
        let path = dir.join(format!("{name}.log"));
        if fs::metadata(&path).is_ok_and(|m| m.len() > 2 * 1024 * 1024) {
            let _ = fs::rename(&path, dir.join(format!("{name}.old.log")));
        }
        Logger { path, gate: Mutex::new(()) }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn write(&self, msg: &str) {
        let _gate = self.gate.lock();
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&self.path) {
            let _ = writeln!(file, "{}  {}", timefmt::stamp(), msg);
        }
    }
}

/// The log file of an app's server.
pub fn server_log(app: &str) -> PathBuf {
    paths::log_dir().join(format!("{app}.log"))
}
