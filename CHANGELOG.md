# Changelog

## Unreleased

- **A new icon:** an engraved octopus on jade green, made by [Roccobot](https://github.com/Roccobot) with ChatGPT Astra 6.2 Max. It has its own drawings for the small sizes, and a version made for each system: the Windows programs, the Mac app and the Dock, and the panel's window on Linux. Its files and their editable sources are in [`assets/icons`](assets/icons).
- **Linux: the control panel is in the app menu.** The install script gives "Craft MCP Hub" an entry in the desktop's app menu and search, with the octopus icon, as Windows has a Start menu shortcut and macOS the app in Applications. `crafthub install-app` does the same for a build from source, and `crafthub uninstall` takes it away again. The panel's window has the entry's name as its app id, so the desktop shows the right icon for it, on Wayland too.

## 0.6.1

- **New versions are announced.** The control panel says when a newer Craft MCP Hub is out, with buttons to open its release on GitHub and to copy the update command. `crafthub update-check` asks from the command line, and `crafthub list` mentions a newer version too. GitHub is asked at most once a day; `CRAFTHUB_NO_UPDATE_CHECK=1` turns it off.

## 0.6.0

- **Apps the launcher doesn't list yet.** The hub has its own short list of them, [`assets/extra-apps.toml`](assets/extra-apps.toml), added after the launcher's catalog. An app the launcher lists, under its id or a former one, is taken from the launcher instead, so an entry steps aside by itself once its app joins. First on it: [ShapeCraft](https://github.com/jackharvest/shapecraft), a 3D modeler, as a community app on port 7821 ([#1](https://github.com/AndrianLeah/craft-mcp-hub/issues/1)).
- **macOS: the apps the ArtCraft launcher installed are found.** The launcher installs only the desktop app (`/Applications/<Name>.app`), and its command-line tool, which runs the MCP engine, is a separate download. The hub now lists those apps, and turning one on (or `crafthub setup`, or `crafthub tools`) fetches the matching tool from the app's release into `~/.crafthub/tools`. It is installed only when it matches the release's checksums and is signed by the app's own developer.
- **macOS: the tools follow the apps' updates by themselves.** When the launcher updates an app, its server downloads the matching tool in the background while the old one keeps working, and switches its engine over once no AI client session is connected, keeping the library it had open. Nothing to do by hand; `crafthub doctor <app>` shows both versions.
- **macOS: Craft MCP Hub is an app.** The install script puts "Craft MCP Hub" in Applications, so it is in Launchpad and Spotlight (also when you search for "crafthub"), and `crafthub` and `crafthub-panel` in `~/.local/bin` point into it. `crafthub install-app` does the same for a build from source.
- **macOS: the icon is the size of the other apps' icons.** It now sits on the macOS icon grid, with the same margin as every other Mac app.
- **An uninstaller:** `crafthub uninstall`, or **Uninstall…** in the panel. It says what it will do and asks first, then turns every app off, takes it out of the AI clients and deletes the hub's app, programs and folder. The ArtCraft apps are left alone.
- A client registration that names the hub by another path to the same program (a link) counts as the hub's own.

## 0.5.1

- ArtCraft is spelled the way its makers spell it, in the programs and the documents.

## 0.5.0

The first public release.

- **One shared server per app.** Each ArtCraft app you turn on gets one windowless engine on its own local port, and every AI client session connects to that same engine. A session starts the server when it isn't running, and closing the control panel stops nothing.
- **A control panel and a command-line tool** (`crafthub`) to turn apps on and off, change their ports, test them with a real MCP round trip, and say why something doesn't connect (`crafthub doctor`, **Diagnose**).
- **The apps come from the ArtCraft launcher's catalog**, so a newly installed app shows up by itself. Forks and custom builds can be added too.
- **AI clients:** Claude Code, Claude Desktop, Cursor, VS Code and Codex are built in. Any other client that keeps its MCP servers in a JSON or TOML file can be described with `crafthub clients add`, and `crafthub guide` tells an AI how to set the hub up for itself.
- **Photo libraries:** engines start with no library, which leaves the desktop app free, and the AI client opens the one it needs (`library_list`, `library_open`, `library_close`).
- **Files:** engines are kept to the folders you choose where the app allows it (PhotoCraft, VectorCraft, FigCraft, WordCraft, PdfCraft, DeckCraft).
- **Security:** servers listen on `127.0.0.1` only and every connection must present a token only you can read. The install scripts check each download against its checksum.
- **Updating:** run the install command again, then `crafthub restart all`.

Tested on Windows with the installed apps. The macOS and Linux builds have not been run against real apps yet: see the README's Status section.
