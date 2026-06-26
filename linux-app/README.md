# Poe2Obsidian — Linux companion app

The native companion app that pairs with the Poe2Obsidian Chrome extension on Linux
(target: Ubuntu 24.04). The extension is a WebSocket *client*; **this app is the
server** it connects to on `ws://127.0.0.1:53122`. The app receives raw Poe.com
DOM HTML plus fetched image bytes, converts them to clean Obsidian Markdown, and
writes them into your vault.

See [`../docs/LINUX_DEVELOPMENT_PLAN.md`](../docs/LINUX_DEVELOPMENT_PLAN.md) for the
roadmap and [`../docs/PROTOCOL.md`](../docs/PROTOCOL.md) for the wire contract.

## Architecture

The core is a **headless, GUI-free Rust crate** (`ravenvault` lib + bin), so the
entire export pipeline (WS server → HTML→Markdown → vault writer → MemPalace) is
testable without any GUI dependencies. A Tauri GUI shell (tray, settings) wraps
this core later (milestone M6) and is the only part needing system WebKit libs.

```
extension ──ws://127.0.0.1:53122──► ravenvault (this crate)
                                        ├─ protocol  (JSON envelope, serde)
                                        ├─ server    (tokio-tungstenite)
                                        ├─ export     (orchestration state machine)
                                        ├─ html2md    (Poe DOM → Markdown)
                                        ├─ vault       (Obsidian writer)
                                        └─ mempalace   (LLM-memory ingest)
```

## Build & test

```bash
cd linux-app
cargo run            # start the headless app
./scripts/check.sh   # fmt + clippy (-D warnings) + tests — the local CI gate
```

Requires a Rust toolchain (`rustup`, no sudo). The headless core needs no system
libraries; only the later Tauri GUI shell needs `webkit2gtk-4.1`.
