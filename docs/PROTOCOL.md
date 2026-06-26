# Poe2Obsidian Extension ↔ Companion App — WebSocket Protocol

Reverse-engineered from the extension source in this repo. This is the **contract the Linux
native app must implement** to be a drop-in replacement for the macOS app. All `type`/`command`
values and field names are quoted verbatim with `file:line` citations.

---

## 0. Transport & constants

- **The app is the WebSocket SERVER**, listening at `ws://127.0.0.1:53122`. The extension is
  the client (`config.js:16`).
- **All messages are JSON text frames.** There are NO binary frames — HTML and file bytes are
  base64-encoded inside JSON (`background.js:862`, `content.js:47-48`).
- **Chunk size:** `CHUNK_SIZE = 1024 * 256` = **262144 bytes (256 KB)** raw, before base64
  (`config.js:44`).
- **Version gate:** `MIN_APP_VERSION = "0.9.1"` (`config.js:54`); extension version `0.10.0`
  (`manifest.json:6`). **The app MUST report `app_version ≥ 0.9.1` or all exports are blocked.**
- **Extension-side timeouts:** connect 5000 ms; quick reconnect 500 ms (`config.js:30,36`);
  `check_destination` reply window **2500 ms** (`background.js:1052`); `get_session_status`
  reply window **250 ms** (`background.js:1347`).

### 0.1 Envelope (every extension→app message)
```json
{
  "version": "1",
  "request_id": "<uuid-v4>",       // keep-alive pings use "keep-alive-<timestamp>"
  "source": "extension",
  "type": "command|request|response|error|ping",
  "command": "<name>",
  "args": { ... }                  // or "result": {...} / "error": {"message": "..."}
}
```
- Replies are correlated by **echoing the same `request_id`** (`background.js:205-207,1054,1349`).
- `type`: `command` = do-something (no strict reply); `request` = expects `response`/`error`;
  `event` = app→ext notice; `ping` = heartbeat.

### 0.2 ⚠️ Result values are stringified
When the extension forwards a content-script result to the app it runs `flattenStringMap()`,
which flattens nested objects to dotted keys and **coerces every value to a String**
(`background.js:391,624-638`). So the app receives e.g. `result: {"scrollTop":"1234",
"atTop":"false"}` — parse accordingly. Conversely, **send real JSON numbers** for values the
extension reads numerically: `scrollSet.value` (`Number(...)`, `background.js:476`;
`content.js:101`), `scrollBy.delta`, `windowSet.x/y`.

---

## 1. Connection lifecycle

### Handshake (mandatory, on `ws.onopen`) — `background.js:122-132`
Extension sends:
```json
{"version":"1","request_id":"<uuid>","source":"extension","type":"command",
 "command":"handshake","args":{"version":"0.10.0"}}
```
App MUST reply (`background.js:210-233`):
```json
{"version":"1","request_id":"<same>","type":"response","command":"handshake",
 "result":{"app_version":"0.9.1","min_extension_version":"0.10.0"}}
```
- If `min_extension_version` > extension version → `EXTENSION_OUTDATED`.
- If `app_version` < `0.9.1` → `APP_OUTDATED`. Either sets `versionCheckError`, which blocks
  exports and forces `session_status.active="false"` (`background.js:215-242,1218-1225`).

### Other lifecycle facts
- **`app_connected` event** (app→ext): extension re-logs active tab URL (`background.js:252-255`).
- **Reconnect is lazy** (`sendToHost` reconnects before sending, `background.js:855-858`). On
  socket close mid-export the extension aborts the session and sends `abort_export`
  (`background.js:163-194`).
- **Launch polling:** after "Launch App", extension polls connect every 500 ms ×60 (~30 s);
  on first OPEN it refocuses the tab and auto-starts the export (`background.js:927-972`).
- **Keep-alive:** content opens a `keepAlive` port (rotates every 25 s, `content.js:333-351`);
  background forwards a `ping` heartbeat (`type:"ping"`, `request_id:"keep-alive-<ts>"`,
  `background.js:1275-1287`). App need not reply; treat as liveness.

---

## 2. Messages FROM extension TO app

| command | type | args | When | Cite |
|---|---|---|---|---|
| `handshake` | command | `{version}` | WS open | `bg:124` |
| `ping` | ping | `{}` | keep-alive ~25s | `bg:1277` |
| `log` | request | `{message,url}` | diagnostics + active-tab URL | `bg:872` |
| `invoke_export` | command | `{tabId,windowId}` | user starts export | `bg:1228` |
| `check_destination` | request | `{}` | before export (expects reply) | `bg:1060` |
| `get_session_status` | request | `{}` | popup/status poll (expects reply) | `bg:1355` |
| `abort_export` | command | `{message,chatTitle?,isBackground?,tabId,windowId,value?,windowX?,windowY?}` | close/abort/dest-error/tab-closed | `bg:182,1144,1530` |
| `request_abort` | command | `{message,tabId,windowId}` | user clicks Cancel | `bg:1412` |
| `reset_timeout` | command | `{}` | export tab regains focus | `bg:573,1442` |
| `update_tab_status` | command | `{isBackground}` | tab visibility change | `bg:1435` |
| `open_result` | command | `{}` | user clicks "Open" | `bg:1423` |
| `capture_complete` | command | `{session:{tabId,windowId},totalChunks,chatTitle}` | after all HTML chunks | `bg:731` |
| `saveDomHtmlChunk` | request | `{chunkBase64,chunkIndex,totalChunks}` | each HTML chunk | `bg:715` |
| `save_file` | request | `{url,chunkBase64,chunkIndex,totalChunks}` | each asset chunk | `bg:780,812` |
| `save_file_complete` | request | `{url}` | last asset chunk | `bg:789,830` |
| `save_file_error` | request | `{url,message}` | asset fetch failed | `bg:841` |

---

## 3. Messages FROM app TO extension (the app orchestrates everything)

Dispatched by `onHostMessage` (`background.js:204-557`).

### `check_destination` response — `bg:1054`
`result.message` non-empty ⇒ destination NOT ready (extension aborts). Return `{}` (no
`message`) to signal ready. Timeout 2500 ms ⇒ assumed OK.

### `session_status` response — `bg:235-249,1364-1383`
All values are **strings**:
```json
{"type":"response","command":"session_status","request_id":"<same>",
 "result":{"active":"true","tabId":"123","windowId":"45","status":"...","current":"10","total":"100"}}
```
Reply within 250 ms. `versionCheckError` overrides this and forces `active:"false"`.

### `capture_start` — `bg:271-332` (app-driven capture)
```json
{"type":"command","command":"capture_start","args":{"session":{"tabId":123,"windowId":45}}}
```
Extension fetches title, shows "Preparing…", streams full page HTML via `saveDomHtmlChunk`,
finishes with `capture_complete`. (`tabId = args.tabId || args.session.tabId`.)

### `download` — `bg:336-345` (asset fetch relay)
```json
{"type":"request","command":"download","args":{"url":"https://.../image"}}
```
Extension fetches in-browser (with Poe cookies) and streams back via `save_file` (§4.2). This
is the **only** way the app gets binary asset bytes.

### Scroll / DOM / window orchestration — `bg:347-415`
`type:"request"` with `command` ∈ {`scrollGetMetrics`, `scrollSet`, `scrollBy`, `domQuery`,
`domClick`, `windowSet`, `stopScroll`, `startKeepAlive`, `stopKeepAlive`, `validatePage`,
`showError`}. Include `args.session` to drive a stateless session (adopted as `app-<tabId>`,
`bg:355-365`). All commands except `stopScroll` wait for the tab to be focused; on resume the
extension emits `reset_timeout` (`bg:375-381,573-576`).

| command | args | result (strings) | content cite |
|---|---|---|---|
| `scrollGetMetrics` | — | `scrollTop,scrollHeight,clientHeight,atTop,documentHidden,acceptsNegative,windowScrollX,windowScrollY` | `content.js:86,207` |
| `scrollSet` | `{value:<num>}` | `{ok,appliedTop}` / `{ok:'false',error:'NO_VALUE'}` | `content.js:99` |
| `scrollBy` | `{delta:<num>}` | `{ok,appliedTop}` | `content.js:108` |
| `domQuery` | `{selector,inContainer?}` | `{count}` | `content.js:116` |
| `domClick` | `{selector,inContainer?}` | `{clicked:0|1}` | `content.js:124` |
| `windowSet` | `{x?,y?}`/`{windowX?,windowY?}` | `{ok,windowScrollX,windowScrollY}` | `content.js:145` |
| `stopScroll` | — | `{ok:true,status:'stopping'}` (no focus wait) | `content.js:141` |
| `startKeepAlive`/`stopKeepAlive` | — | `{ok:true}` | `content.js:133,137` |
| `validatePage` | — | `{ok:'true'}` / `{ok:'false',error:'INVALID_PAGE',message}` | `content.js:60` |
| `showError` | `{message}` | `{ok:'true'}` (renders error toast) | `content.js:81` |

**Scroll container:** `.ChatMessagesScrollWrapper_scrollableContainerWrapper__x8H60`, else the
largest scrollable element (`content.js:176-197`). `acceptsNegative` = column-reverse
container; `atTop` computed vs `clientHeight - scrollHeight` when negative (`content.js:199-222`).
**The app implements the scroll loop:** `scrollGetMetrics` → `scrollSet`/`scrollBy` toward top
→ wait → until `atTop:"true"`, counting messages via `domQuery`.

### UI-driving commands — `bg:257-556`
`close_connect_tab` (closes `ravenvault.app/connect/*` tabs) · `cancel_session` ·
`abort_export` (shows error, **restores scroll/window** if `value`/`windowX`/`windowY`
present) · `focus_tab {tabId,windowId?}` · `update_ui {ui:{type,message,...}}` (`type:'success'`
clears session) · `ui_render {ui:{...}}`. Include a tab id resolvable via
`args.tabId || args.session.tabId`.

### Generic response / error — `bg:417-448`
A bare `type:"response"` with a resolvable `tabId` + optional `result.message`/
`result.showOpenButton:"true"` → success toast + optional Open button, clears session. A bare
`type:"error"` → "Export failed: <error.message>" toast, clears session.

---

## 4. File / asset transfer

Both flows are **JSON text + base64**; reassemble by `request_id` (HTML) or `url` (assets).

### 4.1 DOM HTML capture (ext → app) — `background.js:674-750`, `content.js:9-58`
1. `preparePageHTML` serializes `<!DOCTYPE…> + documentElement.outerHTML` → returns `{size}`.
2. `totalChunks = ceil(size / 262144)`.
3. Per chunk → `saveDomHtmlChunk {chunkBase64,chunkIndex,totalChunks}`. **All chunks share the
   SAME `request_id`** — order by `chunkIndex`.
4. `capture_complete {session,totalChunks,chatTitle}` — note `chatTitle` here carries the
   filename/title (`bg:738`).
5. Extension sends `clearPageHTML` to free the blob.

No per-chunk acks. The captured payload is the **rendered Poe DOM** — the app parses it.

### 4.2 Asset download (app → ext → app) — `background.js:752-850`
1. App sends `download {url}`.
2. Extension `fetch(url,{cache:"force-cache",credentials:'omit',referrer:'https://poe.com/'})`.
3. `totalChunks = ceil(size/262144) || 1`; **one `request_id` for the whole file**.
4. Empty file ⇒ one `save_file {chunkBase64:"",chunkIndex:0,totalChunks:1}` + `save_file_complete`.
5. Else per chunk: `save_file {url,chunkBase64,chunkIndex,totalChunks}`.
6. `save_file_complete {url}` after the last chunk; on error `save_file_error {url,message}`.

---

## 5. Capture data model — the app does ALL parsing

**The extension does NOT parse messages/code/tables/images.** It ships the raw rendered Poe
HTML + `document.title`. The app must extract everything from the HTML. Known selectors for
message detection (`content.js:64-71`): `[class*="ChatMessagesView"]`,
`[class*="ChatMessagesScrollWrapper"]`, `[class*="Message_row"]`,
`[class*="ChatMessage_chatMessage"]`, `[data-message-id]`, `div[id^="message-"]`.

The app is responsible for: HTML→Markdown (messages, fenced code, MD tables, inline
formatting), asset acquisition (find poecdn URLs → `download` → store), filename derivation
from title (sanitized), vault writing, the export state machine + timeouts, restoring page
position on abort, and `open_result`.

---

## 6. externally_connectable / onboarding — `manifest.json:22-26`, `background.js:1292-1305`

- ravenvault.app pages send `{type:'ping'}`; extension replies `{type:'pong'}`, calls
  `connectWebSocket()`, and removes the originating onboarding tab.
- App can send `close_connect_tab` to close `ravenvault.app/connect/*` tabs after pairing.
- When the app is unreachable, the popup opens `ravenvault://open` (the **Linux app must
  register this URL scheme**) and starts launch-polling (`popup.js:139-150`, `bg:1392-1402`).

---

## 7. Gotchas for the Linux implementer
- **App is the WS server** (bind `127.0.0.1:53122`); extension is the client.
- **Everything is JSON text + base64**; never expect binary frames.
- **Result values arrive as strings**; send real numbers for `scrollSet`/`scrollBy`/`windowSet`.
- **Shared `request_id` per HTML doc; `url` per asset** = reassembly keys.
- **Report `app_version ≥ 0.9.1`** or exports are blocked.
- Reply to `check_destination` <2.5 s and `get_session_status` <250 ms.
- Register `ravenvault://open` and answer ravenvault.app onboarding for auto-launch + install
  detection.
