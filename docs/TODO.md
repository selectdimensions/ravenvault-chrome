# RavenVault Linux — TODO / future work

Tracked follow-ups after the initial M0–M7 build. Roughly priority-ordered.

## Conversion quality (M3) — highest priority

The Poe DOM selectors were written from documented class names, not a real
capture. The first live export (`Counting Song Values.md`) revealed:

- [ ] **Role detection is wrong** — the human's message was labelled
  `🤖 Assistant`. Need to identify how Poe marks human vs bot bubbles in the real
  DOM and fix `detect_role` in `html2md.rs`. (Capture HTML with
  `RAVENVAULT_DUMP_HTML` and tune against it.)
- [ ] **Code blocks not fenced** — Azure CLI commands rendered as plain text;
  `<pre><code>` detection didn't match Poe's real markup, and line
  continuations (`\`) were lost. Fix code-block extraction + language detection.
- [ ] **Message splitting** — output looked like one giant section; verify
  `find_message_blocks` splits per message on the real DOM (not the whole view).
- [ ] Add real captured-HTML fixtures to `gui`/core tests and snapshot them
  (golden tests with `insta`) so future Poe DOM changes are caught.
- [x] **Roles fixed** — human = `rightSideMessageWrapper`, bot = `BotMessageHeader`.
- [x] **Code blocks fixed** — `MarkdownCodeBlock_container` → fenced with language;
  Copy/header/footer chrome stripped; render only from the `Prose_presets_prose` body.
- [ ] **Long code blocks are collapsed in Poe's DOM** (`MarkdownCodeBlock_expandButton`)
  so only the visible portion is captured. The extension should click "expand" on
  all collapsibles before `capture_start` to get full code.
- [ ] Re-export to verify after each fix.

## Bulk export — "Export All" (planned feature)

Goal: export every Poe conversation hands-off, instead of clicking each chat.
**Approach chosen: extend the extension + app** to loop over the chat list using
the user's live, logged-in browser session. **Gated on the M3 conversion fix**
(don't mass-produce broken notes).

Flow (reuses M2–M4 unchanged):
1. **Enumerate chats** — read Poe's sidebar/history (`poe.com/chat/...` links),
   scroll-scraping the list to lazy-load all of them. *Main unknown:* whether
   the sidebar exposes the full history or only recent — investigate the real
   DOM / look for a Poe history endpoint. Centralize selectors like M3.
2. **Per chat**: navigate the tab to the URL → run the existing capture flow →
   write note → next. Human-paced to avoid anti-automation.
3. **Resume/idempotent**: vault writer upserts by `uid`, so re-runs skip/update,
   never duplicate. One chat failing must not abort the batch (collect errors,
   report a summary).
4. **Progress UI**: "Exporting 12/137…", cancel button, final summary.

New pieces required:
- Extension (user's fork): an "Export All" action; a command to enumerate the
  chat list; a command to navigate the tab to a chat URL.
- App: a `bulk_export` orchestration wrapping `run_export` in a loop with
  progress + error aggregation.

Investigation first: capture Poe's sidebar HTML and confirm enumeration is
reliable before committing to the loop.

## Metadata

- [ ] Populate `source_url` in the note frontmatter — the extension already logs
  the active tab URL via `log` messages; capture and thread it into `ExportNote`.
- [ ] Populate `created` (timestamp) in frontmatter.
- [ ] Consider per-message timestamps if present in the DOM.

## App / runtime

- [ ] **Live config reload** — currently editing the vault/settings requires a
  restart. Make `AppContext` reload `config.json` (e.g. watch the file or reload
  per export) so the GUI "Save" applies without a restart.
- [ ] **Tray on vanilla GNOME** — works on Ubuntu (AppIndicator preinstalled);
  test on stock GNOME and confirm the window-only fallback is acceptable.
- [ ] GUI: surface live export progress + a success/"Open note" action (today
  status only goes to the extension overlay and logs).
- [ ] GUI: deep-link plugin so a running app receives `ravenvault://` directly.
- [ ] Decide the default form factor: systemd headless service vs. autostart GUI
  (they conflict on port 53122 — document/guard against both running).

## Packaging (M7)

- [ ] Actually build the snap (`snapcraft` + LXD) and iterate the `launcher`
  library paths under confinement (see `docs/SNAP.md`).
- [ ] Run `snap-review` and submit; track the classic-confinement manual review.
- [ ] Add `arm64` to `platforms` once tested.
- [ ] Evaluate the strict-confinement variant (home-folder-only) as an
  auto-publishing alternative.
- [ ] App icon: replace the 128px extension icon with a proper multi-size set
  (incl. an SVG / 256px) for crisp tray + store listing.

## Robustness

- [ ] Scroll loop: tune for very long conversations and lazy-load timing; the
  stall/atTop heuristics may need adjustment against real pages.
- [ ] Handle `update_tab_status` / `reset_timeout` to pause the inactivity clock
  when the export tab is backgrounded (currently advisory/ignored).
- [ ] Restore scroll/window position on abort (send `value`/`windowX`/`windowY`
  in `abort_export`, per PROTOCOL.md §3.8 / §7).
- [ ] `open_result` handling so the extension's "Open" button opens the note.

## Testing / CI

- [ ] GitHub Actions workflow running `scripts/check.sh` (core always; GUI when
  webkit is available).
- [ ] More integration tests covering cancel mid-scroll and mid-download.

## Done (for reference)
- M0–M7 built, tested, committed; headless service + Tauri GUI; manual MemPalace
  ingest (CLI `ingest` + tray); cancel handling; HTML dump for debugging.
