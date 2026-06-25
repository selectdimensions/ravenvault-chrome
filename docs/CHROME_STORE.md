# Publishing the Poe2Obsidian extension (Chrome Web Store — Unlisted)

The extension is rebranded as **Poe2Obsidian** (no `key`, no
`externally_connectable`, v0.11.0). This guide covers an **Unlisted** listing
(installable by link, not searchable) — the lightest path for a companion
extension.

## 1. Build the upload ZIP

```bash
./scripts/package-extension.sh
# -> dist/poe2obsidian-extension-0.11.0.zip   (only the extension files)
```

The ZIP contains exactly: `manifest.json`, `background.js`, `content.js`,
`popup.html`, `popup.js`, `config.js`, `ui-constants.js`, `LICENSE`, `icons/`.

## 2. One-time: developer account

- Go to https://chrome.google.com/webstore/devconsole
- Register as a developer (one-time **US$5** fee). Use a Google account you
  control (e.g. selectdimensions@gmail.com).

## 3. Create the item

1. **New item** → upload `dist/poe2obsidian-extension-0.11.0.zip`.
2. After it processes, set **Visibility → Unlisted**.
3. Fill the store listing (draft copy below).
4. **Save draft → Submit for review.** Unlisted items still get an automated
   (usually fast) review.

> Note on identity: this is a fork of the MIT-licensed **RavenVault** extension.
> Publishing it as **Poe2Obsidian** (new name, new key, your homepage) avoids
> impersonation. Keep the `LICENSE` file and credit RavenVault in the listing
> description (done below) to satisfy MIT attribution.

## 4. Listing copy (draft)

**Name:** Poe2Obsidian

**Summary (≤132 chars):**
Export your Poe.com AI chats to an Obsidian vault as clean Markdown — one chat or your whole history.

**Description:**
```
Poe2Obsidian saves your Poe.com conversations to your local Obsidian vault as
clean Markdown — including code blocks, tables, and images.

• Export the chat you're viewing, or "Export ALL chats" to archive your entire
  Poe history.
• Files are written locally by a small companion app on your own computer; your
  conversations never leave your machine.

Requires the Poe2Obsidian companion app (Linux), which runs a local server the
extension talks to. See the project page for setup.

Open source (MIT). Based on the open-source RavenVault extension.
```

**Category:** Productivity
**Language:** English

**Single purpose (required field):**
```
Export the user's own Poe.com conversations to local Markdown files in their
Obsidian vault.
```

## 5. Permission justifications (required)

| Permission | Justification to paste |
|---|---|
| `activeTab` | Read the current Poe.com chat the user explicitly chose to export when they click the toolbar icon. |
| `scripting` | Inject the content script that captures the conversation DOM and the status overlay, and (on the chat-history page) enumerate the user's chats for "Export All". |
| `tabs` | Detect when the export tab navigates/closes, and navigate the tab between chats during "Export All". |
| Host `https://poe.com/*`, `https://*.poe.com/*` | The extension only operates on Poe.com chat pages. |
| Host `https://poecdn.net/*`, `https://*.poecdn.net/*` | Download images/attachments referenced in conversations so they can be saved alongside the Markdown. |
| Host `http://127.0.0.1/*`, `http://localhost/*` | Talk to the local companion app (WebSocket on 127.0.0.1:53122) that writes the files. No remote servers are contacted. |

**Remote code:** No. All code is in the package; nothing is fetched/eval'd.

## 6. Privacy / data use (required disclosures)

Declare **no data collected**. Talking points:
- The extension sends captured conversation content **only** to a local app on
  `127.0.0.1` (the user's own machine). No external/analytics servers.
- No browsing history, no personal data sold or transferred.
- Check the three "I certify…" data-use boxes accordingly.

A hostable privacy policy is in `PRIVACY.md` (publish it as a URL — e.g. the
GitHub raw link or a GitHub Pages page — and paste that URL in the console).

## 7. After approval

The console shows an **install link** (and an extension ID). Share/use that link
to install. Pair it with the companion app:
```bash
cd linux-app && ./packaging/install.sh      # headless service, or run the GUI
```

## Updating later
Bump `version` in `manifest.json`, re-run `./scripts/package-extension.sh`,
upload the new ZIP to the same item, submit.
