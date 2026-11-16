# Craft MCP Hub

One shared, windowless MCP server per [Artcraft](https://getartcraft.com) app, for every AI client session.

> Unofficial. This project is not affiliated with Artcraft or its apps.

The Artcraft apps (PhotoCraft, VectorCraft, LightCraft, PdfCraft, FigCraft, WordCraft, DeckCraft, GridCraft, Craft Anything, …) each ship a command-line tool with an `mcp` command, so an AI client such as Claude Code can drive them. Wiring that up by hand has rough edges:

- you either keep each desktop app open, or every client session starts its own copy of the engine;
- with one engine per session, sessions can't see each other's documents, and a photo library can only be opened by the first session (the others fail with "library is already open");
- every app has to be registered in the client by hand, and again when an app is renamed or moved.

Craft MCP Hub fixes that. For each app you turn on it runs **one engine, without a window, on its own local port**, and every client session connects to that same engine. A control panel (and a command-line tool) turns apps on and off, picks ports, and tells you why something doesn't connect.

## How it works

```
AI client session ──stdio──▶ crafthub connect photocraft ─┐
AI client session ──stdio──▶ crafthub connect photocraft ─┼─▶ crafthub serve photocraft ──▶ photocraft-cli mcp
control panel / crafthub ────────────────────────────────┘        (127.0.0.1:7801)            (one engine)
```

- `crafthub connect <app>` is what the client launches. It starts the app's server if none is running, so nothing has to be open beforehand, and reconnects on its own when the server restarts.
- `crafthub serve <app>` is the shared server. It keeps the engine running, forwards each session's requests and returns the answers to whoever asked.
- The list of apps comes from the Artcraft launcher's own catalog, so a new app shows up as soon as the launcher knows it.

## Install

**Windows** (PowerShell):

```powershell
irm https://raw.githubusercontent.com/AndrianLeah/craft-mcp-hub/main/install.ps1 | iex
```

**macOS and Linux:**

```sh
curl -fsSL https://raw.githubusercontent.com/AndrianLeah/craft-mcp-hub/main/install.sh | sh
```

The programs aren't code-signed. Installing with the commands above avoids the "unknown publisher" warnings, because those are only shown for files downloaded with a browser. If you download a release by hand, Windows asks once ("More info" → "Run anyway") and macOS needs System Settings → Privacy & Security → "Open Anyway".

You can also [build from source](#build-from-source).

## Use

```sh
crafthub setup     # add and turn on every installed Artcraft app
crafthub panel     # open the control panel
```

Then open a **new** session in your AI client: the apps are there.

| Command | What it does |
| --- | --- |
| `crafthub list` | Every app: installed or not, its port, and what its server is doing |
| `crafthub setup` | Add and turn on every installed app that isn't in the hub yet |
| `crafthub add <app>` | Add one app, for example one you just installed |
| `crafthub on <app>` / `off <app>` | Turn an app on or off (`all` for every added app) |
| `crafthub port <app> <port>` | Move an app's server to another port |
| `crafthub check [<app>]` | A full MCP round trip through the server, as a client would do |
| `crafthub doctor <app>` | Check everything between the client and the engine, and say what to do |
| `crafthub restart <app>` | Restart an app's server |
| `crafthub close-library <app>` | Close the photo library an app has open |
| `crafthub root <app> <folder>` | Set the folder an app's engine may read and write |
| `crafthub snippet <app>` | Print the MCP server entry, for a client you set up by hand |
| `crafthub catalog --update` | Fetch the current app list from getartcraft.com |

Closing the panel stops nothing: the servers keep running until you turn an app off.

### A newly installed app

Install it with the Artcraft launcher, then press **Add** in the panel (it is marked NEW), or run `crafthub add <app>`. `crafthub setup` adds all new ones at once.

### Custom builds and forks

A fork or your own build of an Artcraft app works as long as its command-line tool has an `mcp` command:

```sh
crafthub add chromaforge --cli "C:\Program Files\Chromaforge\chromaforge-cli.exe" --name Chromaforge
```

The hub reads the tool's `--help` to work out how to start it. To start it exactly like a known app instead, add `--like lightcraft`. For a photo app, `--settings <folder name>` says where it lists its recent libraries. The panel has the same under **Add custom app…** (you can drop the file on the window).

### Ports

Each app has a default port (7801 and up, see [`assets/profiles.toml`](assets/profiles.toml)). Change it in the panel or with `crafthub port`. Open sessions follow the server to its new port.

### Photo libraries

LightCraft-style engines choose their library when they start and can't switch afterwards, and a library can be open in only one program at a time. So the hub starts them with **no library**, which leaves the desktop app free, and gives the AI client three extra tools: `library_list`, `library_open` and `library_close`. Opening a library restarts the engine with it, for every session. **Release** in the panel (or `crafthub close-library`) closes it again so the desktop app can open it.

### Other MCP clients

The hub registers apps in **Claude Code** by itself. For any other client, `crafthub snippet <app>` prints the entry to add to its MCP settings; the server still starts on demand.

## When something doesn't connect

`crafthub doctor <app>` (or **Diagnose** in the panel) checks, in order: the app's program, the registration in the client, the server, the engine, a real MCP round trip, and the errors the client logged in the last day. It ends with what to do.

Settings, the access token and logs are in `~/.crafthub` (`%USERPROFILE%\.crafthub` on Windows).

## Security

The servers listen on `127.0.0.1` only. Every connection must first present a token stored in `~/.crafthub/token`, readable only by you: without it, any local program, or a web page making your browser post to localhost, could drive the engines.

The engines run with the access their own profile gives them. PhotoCraft's headless engine only reads and writes inside one folder (your home folder by default; change it with `crafthub root`).

## Status

| System | State |
| --- | --- |
| Windows | Tested with the installed apps |
| macOS | Builds; where the apps keep their command-line tools is a best guess and needs testing |
| Linux | Builds; same as macOS, including Flatpak installs |

If you use macOS or Linux, the output of `crafthub list` and `crafthub doctor <app>` is exactly what is needed to fix the app locations in [`src/discover.rs`](src/discover.rs). Apps whose start-up arguments haven't been verified are marked in [`assets/profiles.toml`](assets/profiles.toml).

## Build from source

With [Rust](https://rustup.rs) installed:

```sh
git clone https://github.com/AndrianLeah/craft-mcp-hub
cd craft-mcp-hub
cargo build --release
```

The programs are `target/release/crafthub` and `target/release/crafthub-panel`; keep them in the same folder. `cargo build --release --no-default-features` builds the command-line tool alone, which needs no display. On Linux the panel needs the usual window-system development packages (see [`.github/workflows/ci.yml`](.github/workflows/ci.yml)).

## License

[MIT](LICENSE)
