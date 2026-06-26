#!/usr/bin/env bash
# Install Poe2Obsidian for the current user — NO sudo required. Builds the release
# binary, installs a user systemd service that runs it on login, and registers
# the `ravenvault://` URL scheme.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
APP_DIR="$(cd "$HERE/.." && pwd)"

BIN_DIR="$HOME/.local/bin"
UNIT_DIR="$HOME/.config/systemd/user"
DESKTOP_DIR="$HOME/.local/share/applications"
CONFIG_DIR="$HOME/.config/ravenvault"

# Bring the rust toolchain onto PATH for non-login shells.
if [ -f "$HOME/.cargo/env" ]; then
    # shellcheck disable=SC1091
    . "$HOME/.cargo/env"
fi

echo "==> Building release binary"
( cd "$APP_DIR" && cargo build --release )

echo "==> Installing files"
mkdir -p "$BIN_DIR" "$UNIT_DIR" "$DESKTOP_DIR" "$CONFIG_DIR"
install -m 0755 "$APP_DIR/target/release/ravenvault" "$BIN_DIR/ravenvault"
install -m 0755 "$HERE/ravenvault-open" "$BIN_DIR/ravenvault-open"
install -m 0644 "$HERE/ravenvault.service" "$UNIT_DIR/ravenvault.service"
install -m 0644 "$HERE/ravenvault.desktop" "$DESKTOP_DIR/ravenvault.desktop"

# Seed a default config the first time only (never clobber the user's settings).
if [ ! -f "$CONFIG_DIR/config.json" ]; then
    cat > "$CONFIG_DIR/config.json" <<'JSON'
{
  "vault_path": "",
  "mempalace_enabled": false,
  "mempalace_bin": "mempalace"
}
JSON
    echo "    Wrote default config: $CONFIG_DIR/config.json (set vault_path!)"
fi

echo "==> Registering ravenvault:// URL scheme"
xdg-mime default ravenvault.desktop x-scheme-handler/ravenvault 2>/dev/null || true
update-desktop-database "$DESKTOP_DIR" 2>/dev/null || true

echo "==> Enabling the background service"
systemctl --user daemon-reload
systemctl --user enable --now ravenvault.service

cat <<EOF

Poe2Obsidian is installed and running for your user.

  Set your vault:   edit ${CONFIG_DIR}/config.json  ("vault_path")
                    then: systemctl --user restart ravenvault
  Status / logs:    systemctl --user status ravenvault
                    journalctl --user -u ravenvault -f
  Stop / disable:   systemctl --user disable --now ravenvault

The Chrome/Chromium extension (repo root) will now find the app on
ws://127.0.0.1:53122. Load it via chrome://extensions -> Load unpacked.
EOF
