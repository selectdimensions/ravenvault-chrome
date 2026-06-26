# Using Poe2Obsidian on Linux

The Linux companion app is the local server the Chrome/Chromium extension talks
to. Install it, point it at your Obsidian vault, load the extension, and export
Poe.com conversations to Markdown.

## 1. Install (no sudo)

```bash
cd linux-app
./packaging/install.sh
```

This builds the release binary, installs it to `~/.local/bin/ravenvault`, sets up
a **user systemd service** that runs it on login, and registers the
`ravenvault://` URL scheme. Nothing requires root.

## 2. Point it at your vault

Edit `~/.config/ravenvault/config.json`:

```json
{
  "vault_path": "/home/you/Obsidian/MyVault",
  "mempalace_enabled": true,
  "mempalace_bin": "mempalace"
}
```

Then restart the service:

```bash
systemctl --user restart ravenvault
```

- `vault_path` — where notes + `attachments/` are written.
- `mempalace_bin` — path to the `mempalace` executable used by manual ingest.

Environment variables override the file: `RAVENVAULT_VAULT`,
`RAVENVAULT_MEMPALACE_BIN`.

### MemPalace ingest is manual

Exports are **never** auto-ingested into MemPalace (that would re-mine the whole
vault on every save). Ingest on demand instead:

```bash
ravenvault ingest            # mine the configured vault into MemPalace
ravenvault ingest /some/dir  # mine a specific folder
```

Or use the **tray menu → "Ingest vault → MemPalace"** in the GUI app.

### Debugging an export

Set `RAVENVAULT_DUMP_HTML` to a folder to save the raw captured page HTML of
each export there (useful for tuning the Markdown conversion):

```bash
RAVENVAULT_DUMP_HTML=/tmp/rv ravenvault
```

## 3. Load the extension

In Chrome/Chromium: `chrome://extensions` → enable Developer Mode → **Load
unpacked** → select the repository root (the folder with `manifest.json`). The
extension will connect to the app on `ws://127.0.0.1:53122`.

## 4. Export

Open a Poe.com chat and click the Poe2Obsidian toolbar icon. The app drives the
scroll-and-capture, converts the page to Markdown, downloads images, and writes
the note into your vault.

## Manage the service

```bash
systemctl --user status ravenvault      # is it running?
journalctl --user -u ravenvault -f      # live logs
systemctl --user restart ravenvault     # after editing config
systemctl --user disable --now ravenvault   # stop + don't autostart
./packaging/uninstall.sh                # remove (keeps your config)
```

## Run without installing (dev)

```bash
RAVENVAULT_VAULT=/path/to/vault cargo run
```

## What's not here yet

- A tray/settings GUI (milestone M6b) — needs `webkit2gtk-4.1` system libraries.
- A Snap Store package (milestone M7) — needs `snapcraft`.

Both require `sudo apt` packages; the service above gives you the full export
pipeline today without them.
