# The icon

An engraved octopus on jade green, made by [Roccobot](https://github.com/Roccobot) with ChatGPT Astra 6.2 Max. Its eight arms reach in different directions and their suckers form a chain of nodes. The ivory-and-ink hatching follows the mascots of the ArtCraft apps, such as [PhotoCraft](https://github.com/storytold/photocraft/blob/main/assets/app-icon/README.md) and [VectorCraft](https://github.com/storytold/vectorcraft/blob/main/assets/app-icon/README.md).

<p>
<img src="linux/hicolor/16x16/apps/craft-mcp-hub.png" width="16" alt="16 px">&nbsp;
<img src="linux/hicolor/24x24/apps/craft-mcp-hub.png" width="24" alt="24 px">&nbsp;
<img src="linux/hicolor/32x32/apps/craft-mcp-hub.png" width="32" alt="32 px">&nbsp;
<img src="linux/hicolor/48x48/apps/craft-mcp-hub.png" width="48" alt="48 px">&nbsp;
<img src="linux/hicolor/64x64/apps/craft-mcp-hub.png" width="64" alt="64 px">&nbsp;
<img src="linux/hicolor/96x96/apps/craft-mcp-hub.png" width="96" alt="96 px">&nbsp;
<img src="linux/hicolor/128x128/apps/craft-mcp-hub.png" width="128" alt="128 px">
</p>

## Where each file is used

| File | Used for |
| --- | --- |
| `windows/craft-mcp-hub.ico` | Built into the Windows programs by [`build.rs`](../../build.rs), so Explorer, the Start menu and the taskbar show it |
| `macos/craft-mcp-hub.icns` | The icon of the Mac app, "Craft MCP Hub" ([`src/macapp.rs`](../../src/macapp.rs)) |
| `macos/craft-mcp-hub-512.png` | The Dock icon when the panel runs outside the Mac app ([`src/bin/crafthub-panel.rs`](../../src/bin/crafthub-panel.rs)) |
| `linux/hicolor/256x256/apps/craft-mcp-hub.png` | The panel's window icon on Windows and Linux, and the project's README |
| `linux/hicolor/` | Laid out as the desktop expects. `crafthub install-app` copies the PNGs up to 512 px into `~/.local/share/icons/hicolor/`, as the icon of the panel's entry in the app menu ([`src/linuxapp.rs`](../../src/linuxapp.rs)) |
| `source/` | The editable drawings the files above are exported from |

When a drawing changes, export the files again under the same names.

## Sources

| File | What it is |
| --- | --- |
| `source/craft-mcp-hub-flat.svg` | The jade square, with 64 px of transparency on every side of the 1024 px board |
| `source/craft-mcp-hub-fullbleed.svg` | Jade to the edges, for applying a mask: the octopus covers about 84% of the width |
| `source/craft-mcp-hub-octopus.svg` | The octopus alone, on transparency |
| `source/small-sizes/` | The drawings redone for small sizes (see below) |

Each of the first three also comes as a 1024 px PNG under the same name. The SVGs are made of editable paths, and in the large drawing the background, the ink and the ivory are separate elements (`background`, `ink`, `ivory`).

## Sizes

A small icon is not the large drawing scaled down: each range of sizes has a drawing of its own.

| Size | Drawing |
| --- | --- |
| 16 px | The silhouette fitted to the pixel grid, with a horizontal pupil (`craft-mcp-hub-16.svg`) |
| 20–24 px | Wider arms and bigger openings (`craft-mcp-hub-20-24.svg`) |
| 32–40 px | The eye's outline and the first suckers (`craft-mcp-hub-32-40.svg`) |
| 48 px | More engraving on the mantle and the arms (`craft-mcp-hub-48.svg`) |
| 64–96 px | Lighter engraving, with merged shadows and a redrawn eye (`craft-mcp-hub-64-96.svg`) |
| 128 px and up | The full engraving |

The ICO holds 16, 20, 24, 32, 40, 48, 64, 96, 128 and 256 px. The ICNS holds eleven images from 16 to 1024 px, Retina densities included, with the plate 824 px wide on the 1024 px board as on Apple's icon grid; at 16 px the plate grows a little to stay legible. The Linux PNGs come in 16, 22, 24, 32, 48, 64, 96, 128, 256, 512 and 1024 px.

## Colors

| Jade green | Ink | Ivory |
| --- | --- | --- |
| `#1BA572` | `#0B0B0C` | `#EFE9DC` |
