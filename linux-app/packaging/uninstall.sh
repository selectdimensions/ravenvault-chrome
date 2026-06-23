#!/usr/bin/env bash
# Remove the per-user RavenVault install (leaves your config + vault untouched).
set -euo pipefail

systemctl --user disable --now ravenvault.service 2>/dev/null || true
rm -f "$HOME/.config/systemd/user/ravenvault.service"
rm -f "$HOME/.local/bin/ravenvault" "$HOME/.local/bin/ravenvault-open"
rm -f "$HOME/.local/share/applications/ravenvault.desktop"
systemctl --user daemon-reload 2>/dev/null || true
update-desktop-database "$HOME/.local/share/applications" 2>/dev/null || true

echo "RavenVault uninstalled. Config preserved at ~/.config/ravenvault/."
