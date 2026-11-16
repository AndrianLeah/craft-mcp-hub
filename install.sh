#!/bin/sh
# Installs craft-mcp-hub (crafthub and crafthub-panel) on macOS or Linux.
#
#   curl -fsSL https://raw.githubusercontent.com/AndrianLeah/craft-mcp-hub/main/install.sh | sh
#
# The programs go to ~/.local/bin (set CRAFTHUB_INSTALL_DIR for another folder).
set -eu

REPO="AndrianLeah/craft-mcp-hub"
BIN_DIR="${CRAFTHUB_INSTALL_DIR:-$HOME/.local/bin}"

case "$(uname -s)" in
  Darwin) os=macos ;;
  Linux) os=linux ;;
  *) echo "Unsupported system: $(uname -s). Build from source: https://github.com/$REPO#build-from-source" >&2; exit 1 ;;
esac
case "$(uname -m)" in
  x86_64 | amd64) arch=x86_64 ;;
  arm64 | aarch64) arch=aarch64 ;;
  *) echo "Unsupported processor: $(uname -m). Build from source: https://github.com/$REPO#build-from-source" >&2; exit 1 ;;
esac
if [ "$os-$arch" = "linux-aarch64" ]; then
  echo "There is no ready-made build for Linux on ARM yet. Build from source: https://github.com/$REPO#build-from-source" >&2
  exit 1
fi

asset="crafthub-$os-$arch.tar.gz"
url="https://github.com/$REPO/releases/latest/download/$asset"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

echo "Downloading $asset..."
if ! curl -fsSL "$url" -o "$tmp/$asset"; then
  echo "Could not download $url" >&2
  exit 1
fi
tar -xzf "$tmp/$asset" -C "$tmp"

mkdir -p "$BIN_DIR"
for bin in crafthub crafthub-panel; do
  if [ -f "$tmp/$bin" ]; then
    install -m 755 "$tmp/$bin" "$BIN_DIR/$bin"
  fi
done

echo "Installed in $BIN_DIR: $("$BIN_DIR/crafthub" --version)"
case ":$PATH:" in
  *":$BIN_DIR:"*) ;;
  *) echo "Add it to your PATH to run it by name:  export PATH=\"$BIN_DIR:\$PATH\"" ;;
esac
echo
echo "Next:  crafthub setup    (adds and turns on every installed Artcraft app)"
echo "       crafthub panel    (opens the control panel)"
