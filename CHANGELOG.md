# Changelog

## 0.5.0

The first public release.

- **One shared server per app.** Each Artcraft app you turn on gets one windowless engine on its own local port, and every AI client session connects to that same engine. A session starts the server when it isn't running, and closing the control panel stops nothing.
- **A control panel and a command-line tool** (`crafthub`) to turn apps on and off, change their ports, test them with a real MCP round trip, and say why something doesn't connect (`crafthub doctor`, **Diagnose**).
- **The apps come from the Artcraft launcher's catalog**, so a newly installed app shows up by itself. Forks and custom builds can be added too.
- **AI clients:** Claude Code, Claude Desktop, Cursor, VS Code and Codex are built in. Any other client that keeps its MCP servers in a JSON or TOML file can be described with `crafthub clients add`, and `crafthub guide` tells an AI how to set the hub up for itself.
- **Photo libraries:** engines start with no library, which leaves the desktop app free, and the AI client opens the one it needs (`library_list`, `library_open`, `library_close`).
- **Files:** engines are kept to the folders you choose where the app allows it (PhotoCraft, VectorCraft, FigCraft, WordCraft, PdfCraft, DeckCraft).
- **Security:** servers listen on `127.0.0.1` only and every connection must present a token only you can read. The install scripts check each download against its checksum.
- **Updating:** run the install command again, then `crafthub restart all`.

Tested on Windows with the installed apps. The macOS and Linux builds have not been run against real apps yet: see the README's Status section.
