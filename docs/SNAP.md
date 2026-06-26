# Building & publishing the Poe2Obsidian snap

The packaging lives in [`linux-app/snap/`](../linux-app/snap/):

- `snapcraft.yaml` — classic-confinement, `core24`, builds the Tauri GUI
  (`ravenvault-gui`) with the Rust plugin and stages the GTK/WebKit runtime.
- `snap/local/launcher` — wires `LD_LIBRARY_PATH`/GDK to the bundled runtime
  (classic snaps don't get the gnome extension's environment).
- `snap/local/poe2obsidian.desktop`, `poe2obsidian.png` — desktop integration.

## Why classic confinement

Poe2Obsidian writes to an **arbitrary, user-chosen Obsidian vault** that may be a
hidden folder or live outside `$HOME`, and it writes continuously in the
background. No auto-connecting strict interface covers that:

- `home` — only non-hidden files under `$HOME`.
- `removable-media` — only `/mnt`,`/media`, and not auto-connected.
- `personal-files` — fixed named paths only; super-privileged (manual review).

So we ship **classic** and justify it at store review. The localhost WebSocket
server itself is fine under strict confinement (snaps share the host network
namespace), so the only blocker is filesystem reach.

### Strict alternative (no manual review)

If you prefer auto-publishing with no review friction, switch to strict and
constrain users to a **visible folder under `$HOME`**:

```yaml
confinement: strict
apps:
  poe2obsidian:
    extensions: [gnome]
    plugs: [home, network-bind]
```

(Drop the `command-chain` launcher — the gnome extension handles the runtime.)
Vaults in hidden dirs or outside `$HOME` won't work in this mode.

## Build (needs `snapcraft` + LXD)

```bash
sudo snap install snapcraft --classic
sudo snap install lxd && sudo lxd init --auto
sudo usermod -aG lxd "$USER"   # then re-login so the group applies

cd linux-app
snapcraft            # builds in an LXD container -> poe2obsidian_0.11.0_amd64.snap
```

On an Ubuntu 24.04 host you can avoid LXD with a host build:

```bash
cd linux-app && snapcraft --destructive-mode
```

Install and test the built snap locally (keeps classic confinement):

```bash
sudo snap install --dangerous --classic poe2obsidian_0.11.0_amd64.snap
poe2obsidian          # launches the tray app
```

## Pre-submission review

```bash
sudo snap install review-tools     # provides snap-review
snap-review poe2obsidian_0.11.0_amd64.snap
```

## Publish to the Snap Store

```bash
snapcraft login
snapcraft register poe2obsidian      # one-time (name must be available/owned)
snapcraft upload --release=stable poe2obsidian_0.11.0_amd64.snap
```

Because the snap is **classic**, the first upload triggers a **manual review**
(~3–5 business days). Reference the justification above in the request.

## Known follow-ups

- The `launcher` library paths are best-effort; verify with
  `sudo snap run --shell poe2obsidian` and adjust if WebKit/GTK can't find a
  library. `snappy-debug` while running surfaces any AppArmor denials (mainly
  relevant if you switch to strict).
- Add `arm64` to `platforms` once tested on that architecture.
