# Craft MCP Hub

Un polpo inciso su verde giada. Le otto braccia seguono direzioni diverse e le ventose disegnano una sequenza di nodi. Il tratteggio avorio e nero riprende il linguaggio delle mascotte di PhotoCraft e VectorCraft.

## Scegliere il file

| Uso | File nel pacchetto |
| --- | --- |
| Base piatta con margine trasparente | `master/craft-mcp-hub-flat.svg` oppure il PNG da 1024 px con lo stesso nome |
| Base verde a tutta superficie per applicare una maschera | `master/craft-mcp-hub-fullbleed.svg` oppure il PNG corrispondente |
| Solo polpo, su trasparenza | `master/craft-mcp-hub-polpo.svg` oppure il PNG corrispondente |
| macOS | `macos/craft-mcp-hub.icns` |
| Windows | `windows/craft-mcp-hub.ico` |
| Linux, vettoriale arrotondata | `linux/hicolor/scalable/apps/craft-mcp-hub.svg` |
| Linux, PNG arrotondate | `linux/hicolor/<taglia>x<taglia>/apps/craft-mcp-hub.png` |
| Sorgenti vettoriali per taglie piccole | `ottiche/` |

La base piatta ha 64 px di trasparenza su ogni lato della tavola da 1024 px. Nella versione a tutta superficie la figura occupa circa l’84% della larghezza, lasciando spazio per la maschera. Gli SVG contengono tracciati modificabili. Nel disegno grande il fondo, l’inchiostro e l’avorio sono elementi distinti.

## Taglie

| Dimensione di destinazione | Disegno |
| --- | --- |
| 16 px | Sagoma rifinita sulla griglia del singolo pixel, con pupilla orizzontale |
| 20–24 px | Tentacoli più larghi e aperture più grandi |
| 32–40 px | Contorno dell’occhio e prime ventose |
| 48 px | Più incisioni sul mantello e sulle braccia |
| 64–96 px | Incisione alleggerita con ombre unite e occhio ridisegnato |
| Da 128 px | Incisione completa |

Il file ICO include 16, 20, 24, 32, 40, 48, 64, 96, 128 e 256 px. Il file ICNS contiene undici rappresentazioni fisiche da 16 a 1024 px, comprese le densità Retina. La placca macOS grande misura 824 px su 1024. A 16 px la placca guadagna spazio per la leggibilità.

I PNG per Linux sono disponibili a 16, 22, 24, 32, 48, 64, 96, 128, 256, 512 e 1024 px. Per installarli per un solo utente, copia il contenuto di `linux/hicolor/` in `~/.local/share/icons/hicolor/` e usa `Icon=craft-mcp-hub` nel file desktop.

## Nel repository

La cartella `repo/assets/` contiene i tre file con i nomi già letti dal progetto. `icon.ico` viene incorporato nella build Windows. `icon-macos-512.png` e `icon-256.png` sono le immagini usate dal pannello per la finestra.

## Colori

| Verde giada | Inchiostro | Avorio |
| --- | --- | --- |
| `#1BA572` | `#0B0B0C` | `#EFE9DC` |

## Riferimenti

- [Catalogo delle app nel launcher ArtCraft](https://github.com/storytold/craft-launcher#the-craft-app-suite)
- [Mascotte e provenienza delle icone nel launcher](https://github.com/storytold/craft-launcher/blob/main/ATTRIBUTION.md)
- [Disegno e palette di PhotoCraft](https://github.com/storytold/photocraft/blob/main/assets/app-icon/README.md)
- [Disegno e griglia di VectorCraft](https://github.com/storytold/vectorcraft/blob/main/assets/app-icon/README.md)
- [Risorsa Windows del progetto](https://github.com/AndrianLeah/craft-mcp-hub/blob/main/build.rs)
- [Caricamento delle icone nel pannello](https://github.com/AndrianLeah/craft-mcp-hub/blob/main/src/bin/crafthub-panel.rs)
