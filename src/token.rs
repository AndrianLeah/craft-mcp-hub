//! The token every connection to a hub server must present first.
//!
//! The servers listen on 127.0.0.1, which any local program can reach, and a web page can make a
//! browser post to it. The token (a file only this user can read) keeps both out.

use crate::paths;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::sync::OnceLock;

static TOKEN: OnceLock<String> = OnceLock::new();

pub fn get() -> &'static str {
    TOKEN.get_or_init(load_or_create)
}

fn read() -> Option<String> {
    let text = fs::read_to_string(paths::token_file()).ok()?;
    let token = text.trim();
    (token.len() >= 32).then(|| token.to_string())
}

fn load_or_create() -> String {
    if let Some(token) = read() {
        return token;
    }
    let _ = fs::create_dir_all(paths::hub_home());
    let mut bytes = [0u8; 32];
    if getrandom::fill(&mut bytes).is_err() {
        // No system randomness: still unguessable enough for a local token, and never reused.
        let seed = format!("{:?}{}", std::time::SystemTime::now(), std::process::id());
        for (i, b) in seed.bytes().cycle().take(bytes.len() * 8).enumerate() {
            bytes[i % 32] = bytes[i % 32].rotate_left(3) ^ b;
        }
    }
    let token: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(paths::token_file()) {
        Ok(mut file) => {
            let _ = file.write_all(token.as_bytes());
            token
        }
        // Another process created it first: use theirs (it may take a moment to be written).
        Err(_) => read()
            .or_else(|| {
                std::thread::sleep(std::time::Duration::from_millis(100));
                read()
            })
            .unwrap_or(token),
    }
}
