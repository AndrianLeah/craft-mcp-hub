//! `crafthub guide`: how an AI agent sets the hub up for itself.
//!
//! The hub can't know every AI client there is. Instead, a person asks their AI to set it up, the
//! AI reads this guide, and tells the hub where it keeps its own MCP servers.

use crate::clients::{self, Snapshot};
use crate::hub::{self, Hub};
use crate::paths;

pub fn text() -> String {
    let hub = Hub::load();
    let registered = Snapshot::take(&hub.config);
    let apps = hub.apps();
    let on: Vec<&str> = apps.iter().filter(|app| hub::state(app, &registered).is_on()).map(|app| app.id.as_str()).collect();
    let installed = apps.iter().filter(|app| app.installed()).count();
    let enabled: Vec<&str> = registered.clients().map(|client| client.id).collect();
    let known: Vec<&str> = clients::all_in(&hub.config).iter().map(|client| client.id).collect();
    let list = |items: &[&str]| if items.is_empty() { "none".to_string() } else { items.join(", ") };

    format!(
        r#"# Craft MCP Hub: guide for AI agents

You are reading this because someone asked you to set up Craft MCP Hub for yourself. It gives you
the ArtCraft apps (image editing, vector illustration, photo library, PDF, interface design,
documents, slides, spreadsheets and more) as MCP servers. Each app runs once on this computer,
without a window, and every AI session shares it.

Everything is done with the `crafthub` command:

    {exe}

Do not edit the hub's own files ({home}) by hand.

## Where things stand

- Apps installed here: {installed}. Apps that are on: {on}.
- AI clients the hub knows: {known}. Enabled: {enabled}.

`crafthub list --json` and `crafthub clients --json` give the details.

## 1. Make yourself a client

The hub registers each app in an AI client's MCP settings as a stdio server that runs
`crafthub connect <app>`.

If you are one of the clients it knows, enable yourself:

    crafthub clients on <id>

If you are not, tell the hub where you keep your MCP servers, then enable yourself:

    crafthub clients add <id> --name "<your name>" --file <full path of your MCP settings file>
    crafthub clients on <id>

Options of `clients add`, when your settings differ from the defaults:

    --format json|toml   the file's format (default: json)
    --key <name>         the object or table that holds the servers
                         (default: mcpServers for json, mcp_servers for toml)
    --typed              your JSON entries carry "type": "stdio"
    --restart            you only read your settings when you start

The hub then changes only its own entries in that file, keeps a backup of it, and refuses a file
it can't parse. From then on it keeps you in step: apps turned on or off later are added to or
removed from your settings too.

If your servers aren't kept in a JSON or TOML file like that, add them yourself. This prints them:

    crafthub snippet                 every app that is on, as an mcpServers object
    crafthub snippet --format toml   the same as [mcp_servers.<app>] tables
    crafthub snippet <app>           one app

## 2. Turn apps on

    crafthub setup        add and turn on every installed app
    crafthub on <app>     one app (`crafthub off <app>` undoes it)

Every app that is on adds its tools to every session, so if the person didn't say which apps they
want, ask rather than turning all of them on.

## 3. Check, then reload

    crafthub check          a full MCP round trip through every app that is on
    crafthub doctor <app>   when one fails: what is wrong, and what to do

Reload your MCP servers (or restart, if you only read your settings at startup), then tell the
person which apps you can now use.

## Good to know

- The servers start on demand. Nothing has to be open, and closing the hub's panel stops nothing.
- Photo apps start with no library open. Call `library_list`, then `library_open` with the one
  the person means; `library_close` releases it for the desktop app.
- Most engines only read and write inside chosen folders (the home folder until the person
  chooses). When a tool answers that a path is "outside the folders", don't work around it: tell
  the person, who can widen it with `crafthub root <app> <folder>`. `crafthub root <app>` shows
  what an app may use now.
- Each app has its own local port: `crafthub port <app> <port>` moves it.
- A fork or custom build of an app: `crafthub add <id> --cli <path to its command-line tool>`.
- Every command: `crafthub --help`, and `crafthub <command> --help`.
"#,
        exe = paths::hub_exe().display(),
        home = paths::hub_home().display(),
        on = list(&on),
        known = list(&known),
        enabled = list(&enabled),
    )
}
