# Poe2Obsidian

Export your Poe.com AI conversations to a local Obsidian vault as clean Markdown
— one chat, or your **entire history** ("Export All"). A Chrome/Chromium
extension captures the conversation in your browser; a small **Linux companion
app** ([`linux-app/`](linux-app/)) running on your machine converts it to
Markdown, downloads images, and writes it to your vault. Nothing leaves your
computer.

> Poe2Obsidian is an open-source fork of the MIT-licensed **RavenVault**
> extension, with a new Linux-native companion app. See [NOTICE.md](NOTICE.md).
> Not affiliated with RavenVault / ravenvault.app.

**Deploy:** [Chrome Web Store guide](docs/CHROME_STORE.md) ·
[Snap Store guide](docs/SNAP.md) · [Linux app usage](linux-app/USAGE.md)

## Installation

Two parts work together — the **extension** (this folder) and the **Linux
companion app** ([`linux-app/`](linux-app/)):

1. **Companion app** — `cd linux-app && ./packaging/install.sh` (no sudo). Then
   set your vault in `~/.config/ravenvault/config.json`. See
   [`linux-app/USAGE.md`](linux-app/USAGE.md).
2. **Extension** — load unpacked (`chrome://extensions` → Developer Mode → Load
   unpacked → this folder), or install the packaged build per
   [`docs/CHROME_STORE.md`](docs/CHROME_STORE.md).

```
┌────────────────────┐                      ┌──────────────────────┐                      ┌─────────────────────────┐
│  Chrome Extension  │  ←── WebSocket ───→  │  Poe2Obsidian app    │  ──────────────────→ │  Obsidian Vault         │
│  (this folder)     │   127.0.0.1:53122    │  (Linux companion)   │                      │  (Markdown + assets)    │
└────────────────────┘                      └──────────────────────┘                      └─────────────────────────┘
```

The extension captures the page; the companion app converts to Markdown,
downloads images, writes to your vault, and can optionally ingest each chat into
[MemPalace](https://github.com/MemPalace/mempalace) (manual). Without the app
running, exports can't complete.

---

## Privacy & Security

Privacy is a core principle.

**All communication stays on your machine.** The extension connects only to a
local WebSocket server (`127.0.0.1:53122`) run by the Poe2Obsidian companion app
on your computer. No data is sent to any external servers.

**No background data collection.** The extension acts only when you click its
icon on a Poe.com page. It does not monitor browsing, collect analytics, or
phone home.

**Fully auditable.** This repository contains the complete, unobfuscated source
for both the extension and the companion app.

---

## Permissions Explained

The extension requests these permissions in `manifest.json`:

| Permission | Why It's Needed |
|------------|-----------------|
| `activeTab` | Access the current Poe.com chat when you click the icon |
| `scripting` | Inject the capture/status content script, and enumerate chats for "Export All" |
| `tabs` | Detect tab navigation/close, and navigate between chats during "Export All" |
| Host: `poe.com` | Run on Poe.com chat pages |
| Host: `poecdn.net` | Download images/attachments from Poe's CDN |
| Host: `localhost` / `127.0.0.1` | Talk to the local companion app |

No browsing history, bookmarks, or downloads permissions are requested.

---

## How It Works

Clicking the icon connects to the local companion app over WebSocket. The app
drives the export — scrolling/capturing the conversation (or enumerating and
walking every chat for "Export All") — and the extension captures page content
(messages, code blocks, tables, images) and fetches assets. All conversion and
file storage happen in the companion app on your machine. See
[`docs/PROTOCOL.md`](docs/PROTOCOL.md) for the wire protocol.

---

## License & Attribution

MIT License. See [LICENSE](LICENSE) and [NOTICE.md](NOTICE.md).

Poe2Obsidian is a fork of the MIT-licensed **RavenVault** extension.
Copyright (c) 2026 RavenVault (original extension).
Modifications (Linux app, Export All, rebrand) © 2026 selectdimensions.