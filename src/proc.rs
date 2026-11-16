//! Running other programs: quietly, captured, or detached.

use std::io;
use std::path::Path;
use std::process::{Command, Stdio};

/// Keeps a console program from opening a console window when we have none ourselves (Windows).
pub fn quiet(command: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

pub struct Output {
    pub ok: bool,
    pub stdout: String,
    /// stdout and stderr together, trimmed.
    pub text: String,
}

/// Runs a program to the end and captures what it printed.
pub fn run<S: AsRef<std::ffi::OsStr>>(program: &Path, args: &[S]) -> Output {
    let mut command = Command::new(program);
    command.args(args).stdin(Stdio::null());
    match quiet(&mut command).output() {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
            let stderr = String::from_utf8_lossy(&out.stderr);
            let text = format!("{stdout}{stderr}").trim().to_string();
            Output { ok: out.status.success(), stdout, text }
        }
        Err(e) => Output { ok: false, stdout: String::new(), text: e.to_string() },
    }
}

/// A command line for people to read, and to compare two commands with.
pub fn display_command(args: &[String]) -> String {
    args.iter()
        .map(|a| if a.is_empty() || a.contains([' ', '\t', '"']) { format!("\"{}\"", a.replace('"', "\\\"")) } else { a.clone() })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Starts a program that outlives us and shares nothing with us: no console, no stdio, its own
/// process group.
pub fn spawn_detached(program: &Path, args: &[String]) -> io::Result<()> {
    let mut command = Command::new(program);
    command.args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    let home = crate::paths::hub_home();
    if home.is_dir() {
        command.current_dir(home);
    }
    detach(command)
}

#[cfg(windows)]
fn detach(mut command: Command) -> io::Result<()> {
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
    keep_std_handles_private();
    // Out of the client's job object when it allows that, so closing the client doesn't take the server along.
    let base = DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP;
    match command.creation_flags(base | CREATE_BREAKAWAY_FROM_JOB).spawn() {
        Ok(_) => Ok(()),
        Err(_) => command.creation_flags(base).spawn().map(|_| ()),
    }
}

/// Children inherit every inheritable handle, and ours include the pipes the AI client talks to us
/// through: a server holding those open would keep the client waiting after we exit.
#[cfg(windows)]
fn keep_std_handles_private() {
    use std::ffi::c_void;
    extern "system" {
        fn GetStdHandle(std_handle: u32) -> *mut c_void;
        fn SetHandleInformation(handle: *mut c_void, mask: u32, flags: u32) -> i32;
    }
    const HANDLE_FLAG_INHERIT: u32 = 1;
    const STD_HANDLES: [i32; 3] = [-10, -11, -12];
    for std_handle in STD_HANDLES {
        // SAFETY: both calls take plain values and fail harmlessly on a handle that isn't valid.
        unsafe {
            let handle = GetStdHandle(std_handle as u32);
            if !handle.is_null() && handle as isize != -1 {
                SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0);
            }
        }
    }
}

#[cfg(unix)]
fn detach(mut command: Command) -> io::Result<()> {
    use std::os::unix::process::CommandExt;
    // SAFETY: setsid is async-signal-safe and touches nothing of ours.
    unsafe {
        command.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    let mut child = command.spawn()?;
    // Reap it if it ends while we are still running.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

/// Ends a process by id, for a server that doesn't answer a shutdown request.
pub fn kill(pid: u32) {
    let pid = pid.to_string();
    if cfg!(windows) {
        let _ = quiet(Command::new("taskkill").args(["/F", "/PID", &pid]).stdout(Stdio::null()).stderr(Stdio::null())).status();
    } else {
        let _ = Command::new("kill").args(["-9", &pid]).stdout(Stdio::null()).stderr(Stdio::null()).status();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_only_what_needs_it() {
        let cmd = vec!["C:\\Program Files\\X\\x-cli.exe".to_string(), "mcp".to_string(), "--headless".to_string()];
        assert_eq!(display_command(&cmd), "\"C:\\Program Files\\X\\x-cli.exe\" mcp --headless");
    }
}
