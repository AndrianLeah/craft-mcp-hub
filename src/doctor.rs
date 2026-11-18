//! `crafthub doctor <app>`: everything between the client and the engine, checked in order, with
//! what to do about what is wrong.
//!
//! The same checks give the report the command line prints and, for the control panel, a list of
//! [`Check`]s: each with how it came out, what was found in a few words, and its one-click fix.

use crate::clients::{self, Registration};
use crate::config::Config;
use crate::control::{self, Probe};
use crate::hub::{self, App, Hub};
use crate::rpc::clip;
use crate::{fetch, logger, paths, proc, timefmt};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// How a check came out. The report marks them `[OK]`, `[!!]`, `[XX]`, `[--]`, `[??]` and `[..]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    Ok,
    /// Works, but worth knowing about.
    Warn,
    Fail,
    /// Not there, or not running, which may be as it should.
    Off,
    /// Could not be found out.
    Unknown,
    /// Still going.
    Pending,
}

/// What a check is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Topic {
    Program,
    Files,
    /// The port, when something else has it.
    Port,
    Server,
    Engine,
    /// The server runs with other settings than the hub's.
    Settings,
    Client,
}

/// The one click that fixes a check that failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fix {
    /// Move the app to this free port.
    Port(u16),
    /// Restart its server.
    Restart,
    /// Turn it on, which registers it with every enabled client.
    TurnOn,
}

/// One line of the checklist the panel shows.
#[derive(Debug, Clone)]
pub struct Check {
    pub topic: Topic,
    /// What the line is called: "Program", "Port 7802", a client's name.
    pub name: String,
    pub mark: Mark,
    /// What was found, in a few words.
    pub found: String,
    pub fix: Option<Fix>,
}

impl Check {
    fn new(topic: Topic, name: impl Into<String>, mark: Mark, found: impl Into<String>) -> Check {
        Check { topic, name: name.into(), mark, found: found.into(), fix: None }
    }

    fn fix(self, fix: Fix) -> Check {
        Check { fix: Some(fix), ..self }
    }
}

/// What the doctor found about an app.
pub struct Diagnosis {
    /// In the order the panel lists them: the program, its files, the port, server and engine,
    /// then each enabled client.
    pub checks: Vec<Check>,
    /// The whole report, as `crafthub doctor` prints it.
    pub report: String,
}

/// The checklist's lines before the checks run, by topic and name: what the panel shows as waiting.
pub fn planned(id: &str) -> Vec<(Topic, String)> {
    let mut lines = vec![(Topic::Program, "Program".to_string()), (Topic::Files, "Files".to_string())];
    if Hub::load().app(id).is_some_and(|app| app.installed()) {
        lines.extend([(Topic::Server, "Server".to_string()), (Topic::Engine, "Engine".to_string())]);
    }
    lines.extend(clients::enabled(&Config::load()).iter().map(|client| (Topic::Client, client.name.to_string())));
    lines
}

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
    diagnose(id, |_| {}).report
}

/// Checks the app, telling `progress` each time more of the checklist is known.
pub fn diagnose(id: &str, mut progress: impl FnMut(&[Check])) -> Diagnosis {
    let Some(app) = Hub::load().app(id) else {
        return Diagnosis { checks: Vec::new(), report: format!("`{id}` is not an app the hub knows. `crafthub list` shows them.") };
    };
    let mut checks = Vec::new();
    let mut todo: Vec<String> = Vec::new();
    let mut out: Vec<String> = Vec::new();
    out.push(format!("DIAGNOSIS  {}  ·  {} UTC", app.name, timefmt::utc(timefmt::now_secs())));
    out.push(String::new());

    program(&app, &mut out, &mut checks, &mut todo);
    progress(&checks);
    files(&app, &mut out, &mut checks);
    progress(&checks);
    // The report has the clients before the server; the checklist has them last.
    let mut client_checks = Vec::new();
    let registered = client(&app, &mut out, &mut client_checks, &mut todo);
    let tail = server(&app, registered, &mut out, &mut checks, &mut todo, &mut progress);
    checks.extend(client_checks);
    progress(&checks);

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
    Diagnosis { checks, report: out.join("\n") }
}

/// Reports which files the engine may use: it applies whether or not a server is running.
fn files(app: &App, out: &mut Vec<String>, checks: &mut Vec<Check>) {
    match app.roots() {
        // Chosen on purpose, so it is reported but not listed as something to fix.
        Some(roots) if roots.is_empty() => {
            out.push(format!(
                "[!!]  Files     no restriction: an AI client can have it open or save any file you can (crafthub root {} <folder> restricts it)",
                app.id
            ));
            checks.push(Check::new(Topic::Files, "Files", Mark::Warn, "any file you can open"));
        }
        Some(roots) => {
            out.push(format!("[OK]  Files     it may read and write only: {}", roots.join(", ")));
            checks.push(Check::new(Topic::Files, "Files", Mark::Ok, format!("only {}", roots.join(", "))));
        }
        None => {
            out.push("[--]  Files     this engine can't be restricted to a folder: an AI client can have it use any file you can".to_string());
            checks.push(Check::new(Topic::Files, "Files", Mark::Off, "it can't be limited to a folder"));
        }
    }
}

fn program(app: &App, out: &mut Vec<String>, checks: &mut Vec<Check>, todo: &mut Vec<String>) {
    match &app.cli {
        Some(cli) => {
            let (program, prefix) = cli.split_first().expect("a found tool has a program");
            let args: Vec<String> = prefix.iter().cloned().chain(["--version".to_string()]).collect();
            let version = proc::run(Path::new(program), &args).text.lines().next().unwrap_or("").to_string();
            out.push(format!("[OK]  Program   {}  ({})", proc::display_command(cli), clip(&version, 60)));
            // The version's first words ("photocraft-cli 0.23.1"), else the program's name.
            let short = version.split_whitespace().take(2).collect::<Vec<_>>().join(" ");
            let name = Path::new(program).file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
            checks.push(Check::new(Topic::Program, "Program", Mark::Ok, if short.is_empty() { name } else { short }));
            fetched_by_hub(app, out);
        }
        None if app.fetchable => {
            out.push(format!("[--]  Program   not downloaded yet: the ArtCraft launcher installs only the desktop app on macOS, and the hub fetches {}'s command-line tool when it is turned on", app.name));
            todo.push(format!("Turn it on, or fetch its tool now: crafthub tools {}.", app.id));
            checks.push(Check::new(Topic::Program, "Program", Mark::Off, "downloaded when you turn it on"));
        }
        None => {
            out.push(format!("[XX]  Program   the command-line tool of {} was not found", app.name));
            todo.push(format!("Install or update {} (the ArtCraft launcher does it), or point the hub at it: crafthub add {} --cli <path>.", app.name, app.id));
            checks.push(Check::new(Topic::Program, "Program", Mark::Fail, "not found"));
        }
    }
}

/// For a tool the hub downloaded: which version, and whether it matches the desktop app.
fn fetched_by_hub(app: &App, out: &mut Vec<String>) {
    let hub = Hub::load();
    let Some(listed) = hub.catalog.get(&app.id) else { return };
    let Some(fetched) = fetch::fetched_version(listed) else { return };
    let in_tools = app.cli.as_ref().and_then(|cli| cli.first()).is_some_and(|p| Path::new(p).starts_with(paths::tools_dir()));
    if !in_tools {
        return;
    }
    match fetch::desktop_version(listed) {
        Some(desktop) if desktop != fetched => out.push(format!(
            "[--]  Update    downloaded tool {fetched}, desktop app {desktop}: the server updates it by itself and switches once no session is connected"
        )),
        _ => out.push(format!("[OK]  Update    downloaded by the hub ({fetched}, same as the desktop app); it follows the app's updates by itself")),
    }
}

/// Reports the registration in each enabled AI client. Returns whether this hub's bridge is
/// registered in at least one.
fn client(app: &App, out: &mut Vec<String>, checks: &mut Vec<Check>, todo: &mut Vec<String>) -> bool {
    let enabled = clients::enabled(&Config::load());
    if enabled.is_empty() {
        out.push("[--]  Client    no AI client is enabled".to_string());
        todo.push(hub::no_client_hint());
        checks.push(Check::new(Topic::Client, "AI clients", Mark::Off, "none is on"));
        return false;
    }
    let mut registered = false;
    let mut turn_on = false;
    for client in enabled {
        match client.registered().map(|servers| clients::registration(&servers, &app.id)) {
            None => {
                out.push(format!("[??]  Client    {}: its settings ({}) could not be read", client.name, client.settings_file().display()));
                checks.push(Check::new(Topic::Client, client.name, Mark::Unknown, "its settings can't be read"));
            }
            Some(Registration::Ours) => {
                out.push(format!("[OK]  Client    {}: registered, sessions connect to port {}", client.name, app.port));
                checks.push(Check::new(Topic::Client, client.name, Mark::Ok, "has it"));
                registered = true;
            }
            Some(Registration::Missing) => {
                out.push(format!("[--]  Client    {}: not registered", client.name));
                checks.push(Check::new(Topic::Client, client.name, Mark::Off, "doesn't have it").fix(Fix::TurnOn));
                turn_on = true;
            }
            Some(Registration::Other(what)) => {
                out.push(format!("[!!]  Client    {}: `{}` is registered by something else ({what})", client.name, app.id));
                checks.push(Check::new(Topic::Client, client.name, Mark::Warn, format!("has another server by this name ({what})")).fix(Fix::TurnOn));
                turn_on = true;
            }
        }
    }
    if turn_on {
        todo.push(format!("Turn it on to register it in every enabled client: crafthub on {}.", app.id));
    }
    registered
}

/// How long ago, in words: "a minute", "5 minutes", "2 hours", "3 days".
fn spoken(since: u64) -> String {
    let seconds = timefmt::now_secs().saturating_sub(since);
    let (n, one, many) = match seconds {
        0..120 => return "a minute".to_string(),
        120..7200 => (seconds / 60, "minute", "minutes"),
        7200..172_800 => (seconds / 3600, "hour", "hours"),
        _ => (seconds / 86_400, "day", "days"),
    };
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {many}")
    }
}

/// Reports the server and its engine. Returns the engine's last stderr lines.
fn server(
    app: &App,
    registered: bool,
    out: &mut Vec<String>,
    checks: &mut Vec<Check>,
    todo: &mut Vec<String>,
    progress: &mut dyn FnMut(&[Check]),
) -> Vec<String> {
    let port = app.port;
    let busy = |checks: &mut Vec<Check>, by: &str| {
        let free = hub::free_port(&Config::load(), None);
        checks.push(Check::new(Topic::Port, format!("Port {port}"), Mark::Fail, format!("used by {by}")).fix(Fix::Port(free)));
        checks.push(Check::new(Topic::Server, "Server", Mark::Off, "not running: it can't start on a busy port"));
    };
    let status = match control::status(port) {
        Probe::Down => {
            let hint = if registered { " (the next client session starts it, and so does `crafthub check`)" } else { "" };
            out.push(format!("[--]  Server    nothing is running on port {port}{hint}"));
            let found = if registered { "not running: the next session starts it" } else { "not running" };
            checks.push(Check::new(Topic::Server, "Server", Mark::Off, found));
            return Vec::new();
        }
        Probe::Foreign(who) => {
            out.push(format!("[XX]  Port {port}  used by {who}"));
            todo.push(format!("Give {} another port: crafthub port {} <port>.", app.name, app.id));
            busy(checks, &who);
            return Vec::new();
        }
        Probe::Up(status) if status.app != app.id => {
            out.push(format!("[XX]  Port {port}  used by the hub server of {}", status.app));
            todo.push(format!("Give {} another port: crafthub port {} <port>.", app.name, app.id));
            busy(checks, &format!("the server of {}", status.app));
            return Vec::new();
        }
        Probe::Up(status) => *status,
    };
    out.push(format!("[OK]  Server    running on port {port} (pid {}, version {}, up {})", status.pid, status.version, timefmt::ago(status.started_at)));
    checks.push(Check::new(Topic::Server, "Server", Mark::Ok, format!("running on port {port}, for {}", spoken(status.started_at))));
    progress(checks);
    // The round trip is what takes time: it goes first, so the engine's line can say how many tools.
    let test = control::check(port);
    let engine = &status.engine;
    match engine.state.as_str() {
        "ready" => {
            out.push(format!("[OK]  Engine    ready: {} (pid {}, restarts: {})", engine.server, engine.pid, engine.restarts));
            match &test {
                Ok(ok) => checks.push(Check::new(Topic::Engine, "Engine", Mark::Ok, format!("ready, with {} tools", ok.tools))),
                Err(why) => checks.push(Check::new(Topic::Engine, "Engine", Mark::Fail, format!("doesn't answer: {why}")).fix(Fix::Restart)),
            }
        }
        // Stopped to free its memory, which is as it should be: the next request starts it. The
        // round trip above started it, and says whether it answers.
        "idle" => {
            out.push(format!(
                "[OK]  Engine    idle: starts on the next request (stopped after {} with no session, to free its memory; the MCP test below starts it)",
                timefmt::span(Duration::from_secs(engine.idle_stop))
            ));
            match &test {
                Ok(_) => checks.push(Check::new(Topic::Engine, "Engine", Mark::Ok, "idle: starts on the next request")),
                Err(why) => checks.push(Check::new(Topic::Engine, "Engine", Mark::Fail, format!("doesn't answer: {why}")).fix(Fix::Restart)),
            }
        }
        "failed" => {
            out.push(format!("[XX]  Engine    does not start: {}", status.last_error));
            if status.last_error.contains("Could not start the engine") {
                todo.push(format!("The engine's program does not start: check that {} is installed, then crafthub restart {}.", app.name, app.id));
            } else {
                todo.push(format!("The {} engine exits at once: see its last lines below. The server retries every 30 seconds.", app.name));
            }
            checks.push(Check::new(Topic::Engine, "Engine", Mark::Fail, format!("doesn't start: {}", status.last_error)).fix(Fix::Restart));
        }
        _ => {
            out.push("[..]  Engine    starting".to_string());
            checks.push(Check::new(Topic::Engine, "Engine", Mark::Pending, "starting"));
        }
    }
    if hub::stale(app, &status) {
        out.push("[!!]  Settings  the server was started with other settings or by another version of the hub".to_string());
        todo.push(format!("Restart it to apply the current settings: crafthub restart {}.", app.id));
        checks.push(Check::new(Topic::Settings, "Settings", Mark::Warn, "it runs with older settings").fix(Fix::Restart));
    }
    if app.profile.library {
        out.push(format!("      Library   {}", status.library.as_deref().unwrap_or("none open (the AI client opens one with library_open)")));
    }
    let names = if status.session_names.is_empty() { String::new() } else { format!(" ({})", status.session_names.join(", ")) };
    out.push(format!("      Sessions  {} connected{names} · {} requests so far", status.sessions, status.requests));
    match test {
        Ok(ok) => out.push(format!("[OK]  MCP test  {} tools in {} ms", ok.tools, ok.millis)),
        Err(why) => out.push(format!("[XX]  MCP test  {why}")),
    }
    status.stderr_tail
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn says_how_long_in_words() {
        let now = timefmt::now_secs();
        assert_eq!(spoken(now - 30), "a minute");
        assert_eq!(spoken(now - 5 * 60), "5 minutes");
        assert_eq!(spoken(now - 2 * 3600 - 60), "2 hours");
        assert_eq!(spoken(now - 3 * 86_400), "3 days");
        assert_eq!(spoken(now - 2 * 86_400), "2 days");
    }
}
