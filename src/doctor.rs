//! `crafthub doctor <app>`: everything between the client and the engine, checked in order, with
//! what to do about what is wrong.

use crate::clients::{self, Registration};
use crate::control::{self, Probe};
use crate::hub::{App, Hub};
use crate::rpc::clip;
use crate::{logger, paths, proc, timefmt};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Where Claude Code keeps its per-project MCP logs.
fn claude_log_base() -> PathBuf {
    if cfg!(windows) {
        paths::cache_dir().join("claude-cli-nodejs").join("Cache")
    } else {
        paths::cache_dir().join("claude-cli-nodejs")
    }
}

struct LoggedError {
    time: String,
    cwd: String,
    text: String,
}

/// What Claude Code logged as errors for this server in the last 24 hours, newest first.
fn claude_errors(app: &str, max: usize) -> Vec<LoggedError> {
    let since = SystemTime::now() - Duration::from_secs(24 * 3600);
    let mut found = Vec::new();
    for project in fs::read_dir(claude_log_base()).into_iter().flatten().flatten() {
        let logs = project.path().join(format!("mcp-logs-{app}"));
        for file in fs::read_dir(logs).into_iter().flatten().flatten() {
            let recent = file.metadata().and_then(|m| m.modified()).is_ok_and(|modified| modified > since);
            if !recent {
                continue;
            }
            for line in fs::read_to_string(file.path()).unwrap_or_default().lines().filter(|l| l.contains("\"error\"")) {
                let Ok(entry) = serde_json::from_str::<Value>(line) else { continue };
                let field = |key: &str| entry.get(key).and_then(Value::as_str).unwrap_or("").to_string();
                let text = field("error");
                // Claude logs the engines' informational stderr as errors too.
                let noise = ["Connection failed (CONNECTION_CLOSED)", "opened library", "serving MCP", "MCP server on stdio", "MCP server ("];
                if text.is_empty() || noise.iter().any(|n| text.contains(n)) {
                    continue;
                }
                found.push(LoggedError { time: field("timestamp"), cwd: field("cwd"), text });
            }
        }
    }
    found.sort_by(|a, b| b.time.cmp(&a.time));
    found.truncate(max);
    found
}

pub fn report(id: &str) -> String {
    let Some(app) = Hub::load().app(id) else {
        return format!("`{id}` is not an app the hub knows. `crafthub list` shows them.");
    };
    let mut out: Vec<String> = Vec::new();
    let mut todo: Vec<String> = Vec::new();
    out.push(format!("DIAGNOSIS  {}  ·  {} UTC", app.name, timefmt::utc(timefmt::now_secs())));
    out.push(String::new());

    program(&app, &mut out, &mut todo);
    let registered = client(&app, &mut out, &mut todo);
    let tail = server(&app, registered, &mut out, &mut todo);

    if !tail.is_empty() {
        out.push(String::new());
        out.push("Last lines from the engine:".to_string());
        out.extend(tail.iter().rev().take(8).rev().map(|line| format!("   {line}")));
    }

    out.push(String::new());
    let errors = claude_errors(&app.id, 6);
    if errors.is_empty() {
        out.push("Claude Code logged no errors for this server in the last 24 hours.".to_string());
    } else {
        out.push("Errors Claude Code logged in the last 24 hours (older ones may be fixed already):".to_string());
        for error in errors {
            let text: String = error.text.split_whitespace().collect::<Vec<_>>().join(" ");
            out.push(format!("   {}  {}", error.time.get(5..16).unwrap_or(&error.time).replace('T', " "), clip(&text, 260)));
            if !error.cwd.is_empty() {
                out.push(format!("                session in {}", error.cwd));
            }
        }
    }

    out.push(String::new());
    out.push("What to do:".to_string());
    if todo.is_empty() {
        out.push("   • Nothing: all is well. Client sessions opened from now on connect to this server.".to_string());
    } else {
        out.extend(todo.iter().map(|t| format!("   • {t}")));
    }
    out.push("   • A client session that was open while a server was not working must be restarted to reconnect.".to_string());
    out.push(String::new());
    out.push(format!("Server log: {}", logger::server_log(&app.id).display()));
    out.join("\n")
}

fn program(app: &App, out: &mut Vec<String>, todo: &mut Vec<String>) {
    match &app.cli {
        Some(cli) => {
            let (program, prefix) = cli.split_first().expect("a found tool has a program");
            let args: Vec<String> = prefix.iter().cloned().chain(["--version".to_string()]).collect();
            let version = proc::run(Path::new(program), &args).text.lines().next().unwrap_or("").to_string();
            out.push(format!("[OK]  Program   {}  ({})", proc::display_command(cli), clip(&version, 60)));
        }
        None => {
            out.push(format!("[XX]  Program   the command-line tool of {} was not found", app.name));
            todo.push(format!("Install or update {} (the Artcraft launcher does it), or point the hub at it: crafthub add {} --cli <path>.", app.name, app.id));
        }
    }
}

/// Reports the registration in Claude Code. Returns whether this hub's bridge is registered.
fn client(app: &App, out: &mut Vec<String>, todo: &mut Vec<String>) -> bool {
    let Some(servers) = clients::registered() else {
        out.push("[??]  Client    Claude Code's settings could not be read just now".to_string());
        return false;
    };
    match clients::registration(&servers, &app.id) {
        Registration::Ours => {
            out.push(format!("[OK]  Client    registered in Claude Code: sessions connect to port {}", app.port));
            true
        }
        Registration::Missing => {
            out.push("[--]  Client    not registered in Claude Code (off)".to_string());
            todo.push(format!("Turn it on: crafthub on {}.", app.id));
            false
        }
        Registration::Other(what) => {
            out.push(format!("[!!]  Client    Claude Code has `{}` registered by something else: {what}", app.id));
            todo.push(format!("Turn it on here to replace that registration: crafthub on {}.", app.id));
            false
        }
    }
}

/// Reports the server and its engine. Returns the engine's last stderr lines.
fn server(app: &App, registered: bool, out: &mut Vec<String>, todo: &mut Vec<String>) -> Vec<String> {
    let port = app.port;
    let status = match control::status(port) {
        Probe::Down => {
            let hint = if registered { " (the next client session starts it, and so does `crafthub check`)" } else { "" };
            out.push(format!("[--]  Server    nothing is running on port {port}{hint}"));
            return Vec::new();
        }
        Probe::Foreign(who) => {
            out.push(format!("[XX]  Port {port}  used by {who}"));
            todo.push(format!("Give {} another port: crafthub port {} <port>.", app.name, app.id));
            return Vec::new();
        }
        Probe::Up(status) if status.app != app.id => {
            out.push(format!("[XX]  Port {port}  used by the hub server of {}", status.app));
            todo.push(format!("Give {} another port: crafthub port {} <port>.", app.name, app.id));
            return Vec::new();
        }
        Probe::Up(status) => *status,
    };
    out.push(format!("[OK]  Server    running on port {port} (pid {}, version {}, up {})", status.pid, status.version, timefmt::ago(status.started_at)));
    let engine = &status.engine;
    match engine.state.as_str() {
        "ready" => out.push(format!("[OK]  Engine    ready: {} (pid {}, restarts: {})", engine.server, engine.pid, engine.restarts)),
        "failed" => {
            out.push(format!("[XX]  Engine    does not start: {}", status.last_error));
            if status.last_error.contains("Could not start the engine") {
                todo.push(format!("The engine's program does not start: check that {} is installed, then crafthub restart {}.", app.name, app.id));
            } else {
                todo.push(format!("The {} engine exits at once: see its last lines below. The server retries every 30 seconds.", app.name));
            }
        }
        _ => out.push("[..]  Engine    starting".to_string()),
    }
    let expected = app.engine_command().map(|command| proc::display_command(&command)).unwrap_or_default();
    if status.version != crate::VERSION || engine.command != expected {
        out.push("[!!]  Settings  the server was started with other settings or by another version of the hub".to_string());
        todo.push(format!("Restart it to apply the current settings: crafthub restart {}.", app.id));
    }
    if app.profile.library {
        out.push(format!("      Library   {}", status.library.as_deref().unwrap_or("none open (the AI client opens one with library_open)")));
    }
    let names = if status.session_names.is_empty() { String::new() } else { format!(" ({})", status.session_names.join(", ")) };
    out.push(format!("      Sessions  {} connected{names} · {} requests so far", status.sessions, status.requests));
    match control::check(port) {
        Ok(ok) => out.push(format!("[OK]  MCP test  {} tools in {} ms", ok.tools, ok.millis)),
        Err(why) => out.push(format!("[XX]  MCP test  {why}")),
    }
    status.stderr_tail
}
