# RavenVault for Linux — Development Plan

**Goal:** Make RavenVault work on Ubuntu 24.04 (and Linux broadly), not just macOS, as a
**native app** — with a real test suite, direct Obsidian-vault writing, MemPalace (LLM
memory) integration, and eventual publication in the **Snap Store**.

**Status:** Planning. Last updated 2026-06-22.

---

## 0. The key realization

This repository (`ravenvault-chrome`) is **only the client half** of RavenVault.

```
┌────────────────────┐                      ┌──────────────────────┐                      ┌─────────────────────────┐
│  Chrome Extension  │  ←── WebSocket ───→  │  RavenVault NATIVE   │  ──────────────────→ │  Obsidian Vault         │
│  (THIS repo)       │      127.0.0.1:53122 │  APP  (NOT in repo)  │                      │  (Markdown + assets)    │
└────────────────────┘                      └──────────────────────┘                      └─────────────────────────┘
                                                       │
                                                       └──────────────────→  MemPalace (LLM memory, ~/.mempalace)
```

- The extension is a WebSocket **client**. It connects to `ws://127.0.0.1:53122` and does
  almost no logic — it ships the **raw rendered Poe DOM HTML** plus fetched image bytes to
  the app and renders status UI.
- **All the real work** — running the WS *server*, HTML→Markdown conversion, asset storage,
  filename/vault logic, scroll orchestration, the export state machine — lives in the
  **native app, which today only exists for macOS (Swift) and is not in this repo.**

**Therefore "make it work on Ubuntu" = build a new Linux-native companion app that speaks
the extension's exact wire protocol.** The extension already runs on Linux Chrome/Chromium
unchanged; only minor extension tweaks (if any) are needed.

The protocol is fully reverse-engineered — see [`PROTOCOL.md`](./PROTOCOL.md) (companion doc).

### Decisions locked in (2026-06-22)
| Decision | Choice | Consequence |
|---|---|---|
| App stack | **Tauri (Rust)** | WS server in-process (tokio), ~3–10 MB bundle, official Snap v2 support |
| Vault portability | **Any vault, anywhere** | Snap **classic confinement** → manual store review (~3–5 days) |
| "mempalace" | **MemPalace LLM memory** (github.com/MemPalace/mempalace) | Ingest exported conversations as agent memory; NOT flashcards |
| MemPalace target | **Local install** at `~/.mempalace` (CLI `~/.local/bin/mempalace`, `mempalace-mcp` MCP server, ChromaDB) | Integrate via CLI/MCP, not a network service |

---

## 1. End state (the vision we plan backwards from)

A polished, installable Ubuntu app that a stranger can `snap install ravenvault` and use:

1. **Native Tauri app** runs in the background with a tray icon; auto-starts on login.
2. Registers `ravenvault://` URL scheme so the extension's "Launch App" deep link works.
3. Runs the WS server on `127.0.0.1:53122`, passes the extension handshake (reports
   `app_version ≥ 0.9.1`), and drives the full app-orchestrated export.
4. Converts captured Poe HTML → clean Obsidian Markdown (messages, code blocks, tables,
   inline formatting), downloads poecdn assets via the extension relay, rewrites links.
5. Writes to **any** user-chosen Obsidian vault (frontmatter with stable `uid`, content-hashed
   image filenames, atomic writes, idempotent upsert).
6. **MemPalace integration:** optionally ingest each exported conversation into the user's
   local MemPalace so it becomes long-term LLM memory.
7. **Comprehensive tests:** unit (HTML→MD, sanitization, protocol serde), async handler
   tests, filesystem tests against a temp vault, a **WS integration test that replays the
   exact extension protocol**, golden/snapshot tests on real Poe fixtures, property tests,
   and snap-confinement smoke tests.
8. **Published to the Snap Store** (classic confinement, passes review), with onboarding that
   matches the extension's install-detection flow.

---

## 2. Backwards plan (end state → first commit)

Each milestone states *what must already be true beneath it*. Read top-down to understand
dependencies; execute bottom-up (§3 gives the forward order).

### M7 — Snap Store published  *(requires M6)*
- `snapcraft.yaml`: `base: core24`, `gnome` extension (`gnome-46-2404`), `confinement: classic`,
  `grade: stable`, `platforms`, `apps`/`parts`/`plugs`.
- Classic-confinement **justification** written (arbitrary/hidden vault paths + continuous
  background writes are not served by any auto-connect interface).
- CI builds the snap, runs `snap-review`/review-tools, smoke-tests with `snappy-debug`.
- Submitted and approved on the stable channel.

### M6 — Production hardening & onboarding  *(requires M5)*
- Tray icon + background/no-window mode (declare `libayatana-appindicator3` dep; **test on
  vanilla GNOME**, graceful fallback to a normal window if no SNI host).
- `ravenvault://open` scheme registered (desktop file `MimeType=x-scheme-handler/ravenvault`).
- Onboarding parity: respond to ravenvault.app `ping` install-detection, drive
  `close_connect_tab`, launch-polling auto-start works end-to-end.
- Auto-start on login, settings UI (vault path picker, MemPalace toggle), error reporting.

### M5 — MemPalace (LLM memory) integration  *(requires M4)*
- Research spike: pin the integration surface of the **local** install — `mempalace` CLI
  store/ingest command vs `mempalace-mcp` stdio JSON-RPC `store` tool (29-tool MCP server).
- Adapter: after a successful export, optionally push the conversation (verbatim, per
  MemPalace's design) into the palace at `~/.mempalace` (collection `mempalace_drawers`).
- Map conversation → MemPalace's Wing/Room model; let MemPalace's own heuristics classify
  (Wings: emotions, consciousness, memory, technical, identity, family, creative).
- Setting to enable/disable; never block the core export if MemPalace is absent/fails.
- Tests: mock the CLI/MCP boundary; integration test against a throwaway palace dir.

### M4 — Obsidian vault writer, production-grade  *(requires M3)*
- YAML frontmatter (stable `uid`/`source`/`url` for idempotent upsert; `tags` without `#`;
  ISO dates; quote colon-space values and internal links).
- Assets → `attachments/` with **content-hashed filenames**; `![[file]]` embeds.
- Filename sanitization (strip `[ ] # ^ | \ / : * " < > ?` + control chars + leading dot;
  neutralize Windows reserved names; NFC-normalize; cap ~200 bytes).
- Atomic writes (temp + rename); don't clobber a note Obsidian has open; collision handling.
- Tests: filesystem (`tempfile`/`assert_fs`), property tests on sanitization & idempotency.

### M3 — HTML → Markdown conversion  *(requires M2)*
- Parse captured Poe DOM HTML (selectors: `[class*="Message_row"]`,
  `[class*="ChatMessage_chatMessage"]`, `[data-message-id]`, `div[id^="message-"]`).
- Convert user/bot messages, fenced code blocks, Markdown tables, inline formatting.
- Identify poecdn asset URLs, issue `download` requests, rewrite links to local paths.
- Tests: **golden/snapshot** (`insta`) over saved real Poe-capture HTML fixtures.

### M2 — Full export orchestration  *(requires M1)*
- Implement the app-driven state machine: `invoke_export` → `startKeepAlive` → scroll loop
  (`scrollGetMetrics`/`scrollSet`/`scrollBy` until `atTop`, count via `domQuery`) →
  `capture_start` → reassemble `saveDomHtmlChunk` (by shared `request_id`) → `capture_complete`.
- Asset relay: `download` → reassemble `save_file`/`save_file_complete`/`save_file_error` (by `url`).
- Progress via `update_ui`/`ui_render`; `check_destination` (reply <2.5s) and
  `get_session_status` (reply <250ms, string-typed fields); inactivity timeout +
  `reset_timeout`; `abort_export` with restore metrics (`value`/`windowX`/`windowY`).
- Remember: result values arrive **flattened to strings**; send real numbers back for
  `scrollSet.value`, `scrollBy.delta`, `windowSet.x/y`.

### M1 — WebSocket server + handshake  *(requires M0)*  ← **first "it talks!" moment**
- Tauri app binds `tokio-tungstenite`/`axum` WS server on `127.0.0.1:53122`.
- Parse the JSON envelope (`version`, `request_id`, `source`, `type`, `command`, `args`).
- Implement `handshake` → reply `app_version: "0.9.1"+`, correlate by `request_id`.
- Handle `ping` keep-alive; basic `log` handling.
- Test: WS integration test replays the real handshake and asserts acceptance.

### M0 — Project skeleton & dev loop  ← **start here**
- New repo/dir `ravenvault-linux` (Tauri + Rust workspace). Decide: sibling repo vs. this repo.
- Load the extension unpacked in Chromium on Ubuntu (the `key` in `manifest.json` pins the
  same extension ID, so onboarding/IDs match).
- CI scaffold (`cargo test`, fmt, clippy). Write `docs/PROTOCOL.md` from the spec.

---

## 3. Forward execution order (what we actually do)

> **Your immediate priority — "working for me now" — is M0 → M3.** That's the minimum to
> click the extension on a Poe chat and get a Markdown file in your vault. Everything after
> is polish, integrations, and shipping to others.

**Phase A — Get it working for you (the MVP):** M0 → M1 → M2 → M3 → M4(basic)
Outcome: on your Ubuntu box, click the extension on `poe.com/chat/...`, the app scrolls,
captures, converts, and writes a Markdown note + images into your vault.

**Phase B — Your integrations:** M5 (MemPalace) + M4 (hardened writer)
Outcome: exports also flow into your local MemPalace as LLM memory.

**Phase C — Make it shippable for everyone:** M6 (tray, scheme, onboarding) → M7 (Snap Store).

---

## 4. Critical risks & how the plan de-risks them

| Risk | Mitigation (where) |
|---|---|
| **Snap can't write to arbitrary vault** under strict confinement | Decided: **classic confinement** + written justification (M7). Localhost server is fine under strict — snaps share the host net namespace + `network-bind`. |
| **No system tray on vanilla GNOME** (Ubuntu 24.04 ships AppIndicator, vanilla GNOME doesn't) | Declare AppIndicator dep, **test on vanilla GNOME**, fall back to a normal window (M6). |
| Poe DOM markup changes break parsing | Golden fixtures + snapshot tests catch drift early (M3); keep selectors centralized. |
| MemPalace API/CLI surface uncertain | Research spike before coding the adapter; integrate against the **local** install; never block export on MemPalace failure (M5). |
| Protocol subtleties (string-flattened results, shared `request_id`, version gate) | Captured in `PROTOCOL.md`; WS integration test replays the real sequence (M1–M2). |

---

## 5. Testing strategy (woven through every milestone)

| Layer | Tools | First appears |
|---|---|---|
| Unit (pure) | `#[test]` — HTML→MD, sanitization, serde envelope | M1/M3/M4 |
| Async + mocks | `#[tokio::test]`, `mockall` | M2 |
| Filesystem | `tempfile`, `assert_fs`, `predicates` | M4 |
| **WS integration (replay extension protocol)** | `tokio-tungstenite` client on `127.0.0.1:0` | M1 (highest value) |
| Golden / snapshot | `insta` (+`glob!`) over real Poe fixtures | M3 |
| Property | `proptest` — sanitize idempotency, chunk round-trip | M4 |
| Snap confinement | `snap try`, `--dangerous`, `snappy-debug`, `snap-review` | M7 |

---

## 6. Immediate next actions (M0)

1. Decide repo layout (recommend a sibling `ravenvault-linux/` Tauri repo; keep this
   extension repo as-is).
2. Scaffold the Tauri + Rust project; add `tokio-tungstenite`, `serde`/`serde_json`, `tokio`.
3. Stand up the WS server on `127.0.0.1:53122` and answer `handshake` with `app_version 0.9.1`.
4. Load the extension unpacked in Chromium on Ubuntu; confirm the handshake succeeds (the
   extension stops reporting "app not running").
5. Write the first WS integration test that replays the handshake.

> When M1 connects successfully, the extension's "app not running" state clears — that's the
> first green light that the Linux port is real.
