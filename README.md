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

**[Install from the Chrome Web Store](https://chromewebstore.google.com/detail/ravenvault/jjcpdcmcellmiaagaihhbgohaidfpmkp)**

This repository contains the complete source code for the Chrome extension. We've open-sourced it so you can audit exactly what runs in your browser and verify that your data stays on your machine.

```
┌────────────────────┐                      ┌──────────────────────┐                      ┌─────────────────────────┐
│  Chrome Extension  │  ←── WebSocket ───→  │  RavenVault macOS    │  ──────────────────→ │  Obsidian Vault         │
│  (this code)       │      localhost       │  App                 │                      │  (Markdown + assets)    │
└────────────────────┘                      └──────────────────────┘                      └─────────────────────────┘
```

---

## Privacy & Security

This extension is designed with privacy as a core principle.

**All communication stays on your machine.** The extension connects only to a local WebSocket server (localhost/127.0.0.1) running on your computer as part of the RavenVault macOS app. No data is sent to any external servers.

**No background data collection.** The extension activates only when you click its icon on a Poe.com page. It does not monitor your browsing, collect analytics, or phone home.

**Minimal permissions.** The extension requests only the permissions necessary to function (see below).

**Fully auditable.** This repository contains the complete, unobfuscated source code. The published Chrome Web Store version is built directly from this source with no additional transformations.

---

## Permissions Explained

The extension requests these permissions in `manifest.json`:

| Permission | Why It's Needed |
|------------|-----------------|
| `activeTab` | To access the current Poe.com page when you click the extension icon |
| `scripting` | To inject the content script that captures the page and displays status UI |
| `tabs` | To detect when you navigate away from or close a tab during export |
| Host permission: `poe.com` | To run on Poe.com chat pages |
| Host permission: `poecdn.net` | To download images and attachments from Poe's CDN |
| Host permission: `localhost` | To communicate with the RavenVault macOS app on your machine |
| `externally_connectable`: `ravenvault.app` | Allows the RavenVault onboarding page to detect when the extension is installed and complete setup automatically |

The extension does not request permissions for browsing history, bookmarks, downloads, or any other sensitive browser data.

---

## How It Works

When you click the extension icon, it connects to the locally-running macOS app via WebSocket. The app coordinates the export by sending commands to scroll through and capture the conversation. The extension captures page content, including messages, code blocks, tables, and images, and fetches any media assets. All processing and file storage happens in the macOS app on your machine.

---

## Verifying the Published Extension

To confirm the Chrome Web Store version matches this source code:

1. Clone this repository to your computer
2. In Chrome, go to `chrome://extensions` and enable Developer Mode
3. Click "Load unpacked" and select the repository folder
4. Compare the loaded extension's behavior and code against the Web Store version

You can also inspect the installed extension's source files directly:

1. Go to `chrome://extensions`
2. Find RavenVault and click "Details"
3. Click "Inspect views" to open DevTools
4. Navigate to the Sources tab to view the running code

The extension contains no build step or minification—what you see in this repository is exactly what runs in your browser. Auditors can directly compare source files in DevTools against this repository to verify they match.

---

## License

MIT License. See [LICENSE](LICENSE) file.

Copyright (c) 2026 RavenVault

---

## Contributions

This repository is provided for transparency and auditing purposes only. We are not accepting pull requests, issues, or feature suggestions on this repository.