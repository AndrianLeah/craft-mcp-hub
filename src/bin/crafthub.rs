//! `crafthub`: the command-line tool, the shared server, and the bridge AI clients launch.

use clap::{Parser, Subcommand};
use crafthub::hub::{self, AddOptions, App, Hub, Origin, StateKind};
use crafthub::server::{self, ServerSpec};
use crafthub::{catalog, clients, doctor, logger, paths, shim};
use serde_json::json;
use std::process::ExitCode;

#[derive(Parser)]
#[command(
    name = "crafthub",
    version,
    about = "Unofficial MCP hub for the Artcraft app suite: one shared, windowless MCP server per app, for every AI client session."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Show every app: installed or not, added or not, and what its server is doing
    #[command(alias = "status")]
    List {
        /// Only this app
        app: Option<String>,
        /// Print JSON
        #[arg(long)]
        json: bool,
    },
    /// Add and turn on every installed app that isn't in the hub yet
    Setup,
    /// Add one app: a newly installed one, or a custom build or fork with --cli
    Add {
        /// The app's id, e.g. `photocraft`; any name you like for a custom app
        id: String,
        /// The app's command-line tool, for a custom build or fork
        #[arg(long)]
        cli: Option<String>,
        /// Display name
        #[arg(long)]
        name: Option<String>,
        /// Start it the way this known app is started, instead of working it out from its --help
        #[arg(long)]
        like: Option<String>,
        /// Port for its server (default: the app's usual port, or the next free one)
        #[arg(long)]
        port: Option<u16>,
        /// The app's settings folder, where its recent libraries are listed (repeatable)
        #[arg(long = "settings")]
        settings: Vec<String>,
        /// Only add it, don't turn it on
        #[arg(long)]
        off: bool,
    },
    /// Turn an app off and forget its settings
    Remove { id: String },
    /// Turn an app on: register it with the AI client and start its server (`all` for every added app)
    On { app: String },
    /// Turn an app off: remove it from the AI client and stop its server (`all` for every added app)
    Off { app: String },
    /// Move an app's server to another port
    Port { id: String, port: u16 },
    /// Set the folder an app's engine may read and write (apps that need one, like PhotoCraft)
    Root { id: String, dir: String },
    /// Test an app's server with a full MCP round trip (`all`, the default, for every app that is on)
    Check { app: Option<String> },
    /// Check everything between the AI client and an app's engine, and say what to do
    Doctor { id: String },
    /// Restart an app's server
    Restart { id: String },
    /// Close the library an app has open, so the desktop app can open it
    CloseLibrary { id: String },
    /// Show where the app catalog comes from, or fetch the current one
    Catalog {
        /// Download the current catalog from getartcraft.com
        #[arg(long)]
        update: bool,
    },
    /// Print the MCP server entry for an app, to paste into a client the hub doesn't set up itself
    Snippet { id: String },
    /// Open the control panel window
    Panel,
    /// Bridge an AI client session to an app's shared server (this is what AI clients run)
    Connect { id: String },
    /// Run an app's shared server (started automatically; you don't need to run it yourself)
    #[command(hide = true)]
    Serve { id: String },
}

fn main() -> ExitCode {
    let ok = match Cli::parse().command {
        Command::List { app, json } => list(app.as_deref(), json),
        Command::Setup => setup(),
        Command::Add { id, cli, name, like, port, settings, off } => add(&id, AddOptions { cli, name, like, port, settings }, off),
        Command::Remove { id } => done(hub::remove(&id), &format!("{id}: removed")),
        Command::On { app } => each(&app, "on", hub::turn_on),
        Command::Off { app } => each(&app, "off", hub::turn_off),
        Command::Port { id, port } => done(hub::set_port(&id, port), &format!("{id}: port {port}")),
        Command::Root { id, dir } => done(hub::set_root(&id, &dir), &format!("{id}: file folder {dir}")),
        Command::Check { app } => check(app.as_deref().unwrap_or("all")),
        Command::Doctor { id } => {
            println!("{}", doctor::report(&id));
            true
        }
        Command::Restart { id } => done(hub::restart(&id), &format!("{id}: restarted")),
        Command::CloseLibrary { id } => done(hub::close_library(&id), &format!("{id}: library closed")),
        Command::Catalog { update } => show_catalog(update),
        Command::Snippet { id } => {
            println!("{}", serde_json::to_string_pretty(&json!({ "mcpServers": { id.as_str(): clients::entry(&id) } })).unwrap_or_default());
            true
        }
        Command::Panel => panel(),
        Command::Connect { id } => return ExitCode::from(shim::run(&id) as u8),
        Command::Serve { id } => return ExitCode::from(serve(&id) as u8),
    };
    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn done(outcome: Result<(), String>, message: &str) -> bool {
    match outcome {
        Ok(()) => {
            println!("{message}");
            true
        }
        Err(why) => {
            eprintln!("{why}");
            false
        }
    }
}

/// The ids `which` stands for: one app, or with `all` every app that has been added.
fn targets(which: &str) -> Vec<String> {
    if which == "all" {
        Hub::load().apps().into_iter().filter(|a| a.configured && a.installed()).map(|a| a.id).collect()
    } else {
        vec![which.to_string()]
    }
}

fn each(which: &str, word: &str, action: fn(&str) -> Result<(), String>) -> bool {
    let ids = targets(which);
    if ids.is_empty() {
        println!("No apps have been added yet: `crafthub setup` adds every installed one.");
    }
    let mut ok = true;
    for id in ids {
        match action(&id) {
            Ok(()) => println!("{id:<16} {word}"),
            Err(why) => {
                println!("{id:<16} FAILED: {why}");
                ok = false;
            }
        }
    }
    ok
}

fn origin(app: &App) -> &'static str {
    match app.origin {
        Origin::Official => "official",
        Origin::Community => "community",
        Origin::Custom => "custom",
    }
}

fn list(only: Option<&str>, as_json: bool) -> bool {
    let hub = Hub::load();
    let apps: Vec<App> = hub.apps().into_iter().filter(|a| only.is_none_or(|id| a.id == id)).collect();
    if apps.is_empty() {
        eprintln!("No such app. `crafthub list` shows every app the hub knows.");
        return false;
    }
    let servers = clients::registered();
    let rows: Vec<(App, hub::AppState)> = apps.into_iter().map(|app| (app.clone(), hub::state(&app, servers.as_ref()))).collect();
    if as_json {
        let items: Vec<_> = rows
            .iter()
            .map(|(app, state)| {
                json!({
                    "id": app.id, "name": app.name, "origin": origin(app), "installed": app.installed(), "added": app.configured,
                    "port": app.port, "state": state.label, "detail": state.detail, "library": state.library,
                    "engine": app.engine_command(), "server": state.status,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&items).unwrap_or_default());
        return true;
    }
    println!("{:<16} {:<6} {:<14} DETAIL", "APP", "PORT", "STATE");
    for (app, state) in &rows {
        let port = if app.port == 0 || state.kind == StateKind::NotInstalled { "-".to_string() } else { app.port.to_string() };
        let mut detail = state.detail.clone();
        if let Some(library) = &state.library {
            detail = format!("{detail} · library: {library}");
        }
        if state.stale {
            detail = format!("{detail} · restart to apply settings");
        }
        if app.origin != Origin::Official {
            detail = format!("{detail} [{}]", origin(app));
        }
        println!("{:<16} {:<6} {:<14} {}", app.id, port, state.label.to_lowercase(), detail.trim_start_matches(" · ").trim());
    }
    let new = rows.iter().filter(|(_, s)| s.kind == StateKind::New).count();
    if new > 0 {
        println!("\n{new} installed app(s) not added yet: `crafthub setup` adds and turns them on.");
    }
    true
}

fn setup() -> bool {
    let results = hub::setup();
    if results.is_empty() {
        println!("Nothing new: every installed app is already in the hub. (`crafthub list` shows them, `crafthub on all` turns them on.)");
        return true;
    }
    let mut ok = true;
    for (app, outcome) in &results {
        let port = Hub::load().app(&app.id).map_or(app.port, |a| a.port);
        match outcome {
            Ok(()) => println!("{:<16} on, port {port}", app.id),
            Err(why) => {
                println!("{:<16} FAILED: {why}", app.id);
                ok = false;
            }
        }
    }
    println!("\nOpen a new AI client session to use them.");
    ok
}

fn add(id: &str, options: AddOptions, stay_off: bool) -> bool {
    let app = match hub::add(id, options) {
        Ok(app) => app,
        Err(why) => {
            eprintln!("{why}");
            return false;
        }
    };
    println!("{}: added on port {}", app.id, app.port);
    println!("  engine: {}", crafthub::proc::display_command(&app.engine_command().unwrap_or_default()));
    if app.profile.library {
        println!("  libraries: opened by the AI client (library_list, library_open); recent ones read from {}", app.profile.settings.join(", "));
    }
    if stay_off {
        return true;
    }
    done(hub::turn_on(id), &format!("{id}: on. Open a new AI client session to use it."))
}

fn check(which: &str) -> bool {
    let servers = clients::registered();
    let ids: Vec<String> = if which == "all" {
        Hub::load().apps().into_iter().filter(|app| hub::state(app, servers.as_ref()).is_on()).map(|a| a.id).collect()
    } else {
        vec![which.to_string()]
    };
    if ids.is_empty() {
        println!("No app is on.");
    }
    let mut ok = true;
    for id in ids {
        match hub::check(&id) {
            Ok(result) => println!("{id:<16} OK      {} tools in {} ms ({})", result.tools, result.millis, result.server),
            Err(why) => {
                println!("{id:<16} FAILED  {why}");
                ok = false;
            }
        }
    }
    ok
}

fn show_catalog(update: bool) -> bool {
    if update {
        match catalog::update() {
            Ok(count) => println!("Downloaded the catalog: {count} apps."),
            Err(why) => {
                eprintln!("{why}");
                return false;
            }
        }
    }
    let catalog = catalog::load();
    println!("{} apps, from {}", catalog.apps.len(), catalog.source);
    for app in &catalog.apps {
        println!("  {:<16} {:<16} {}", app.id, app.name, app.tagline);
    }
    true
}

fn panel() -> bool {
    let exe = paths::panel_exe();
    if !exe.is_file() {
        eprintln!("The control panel ({}) is not installed next to crafthub.", exe.display());
        return false;
    }
    match std::process::Command::new(&exe).spawn() {
        Ok(_) => true,
        Err(e) => {
            eprintln!("Could not open the control panel: {e}");
            false
        }
    }
}

fn serve(id: &str) -> i32 {
    let log = logger::Logger::new(id);
    let Some(app) = Hub::load().app(id) else {
        log.write(&format!("`{id}` is not an app the hub knows"));
        return 2;
    };
    let Some(engine) = app.engine_command() else {
        log.write(&format!("{} is not installed: its command-line tool was not found", app.name));
        return 2;
    };
    server::run(ServerSpec { app: app.id.clone(), port: app.port, engine, library_tools: app.profile.library, recent: app.recent_files() })
}
