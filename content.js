/*
 * MIT License
 * Copyright (c) 2026 RavenVault
 * See LICENSE file for details.
 */

let cachedBlob = null;

async function handle(req) {
  if (req && req.command === 'preparePageHTML') {
    const doc = document
    let doctype = '<!DOCTYPE html>'
    const dt = doc.doctype
    if (dt) {
      const name = dt.name || 'html'
      const pub = dt.publicId || ''
      const sys = dt.systemId || ''
      if (pub && sys) {
        doctype = `<!DOCTYPE ${name} PUBLIC "${pub}" "${sys}">`
      } else if (pub) {
        doctype = `<!DOCTYPE ${name} PUBLIC "${pub}">`
      } else if (sys) {
        doctype = `<!DOCTYPE ${name} SYSTEM "${sys}">`
      } else {
        doctype = `<!DOCTYPE ${name}>`
      }
    }
    const htmlEl = doc.documentElement
    const html = (doctype + '\n' + (htmlEl ? htmlEl.outerHTML : '')).trim()
    
    cachedBlob = new Blob([html], { type: 'text/html' });
    return { size: String(cachedBlob.size) }
  }

  if (req && req.command === 'getPageHTMLChunk') {
      if (!cachedBlob) return { error: 'NO_BLOB' };
      const args = req.args || {};
      const offset = args.offset || 0;
      const size = args.size || (1024 * 1024);
      const slice = cachedBlob.slice(offset, offset + size);
      
      const b64 = await new Promise((resolve) => {
          const reader = new FileReader();
          reader.onloadend = () => {
              const res = reader.result;
              // strip data:text/html;base64,
              const b64 = res.split(',')[1];
              resolve(b64);
          };
          reader.readAsDataURL(slice);
      });
      return { chunkBase64: b64 };
  }

  if (req && req.command === 'clearPageHTML') {
      cachedBlob = null;
      return { ok: true };
  }

  if (req && req.command === 'validatePage') {
    // Structural check for Poe Chat elements
    // We check for multiple possible indicators to handle different chat types (Single, Group, Bot)
    // and to be robust against minor class name changes.
    const selectors = [
        '[class*="ChatMessagesView"]',           // Original check
        '[class*="ChatMessagesScrollWrapper"]',  // Original check
        '[class*="Message_row"]',                // Original check
        '[class*="ChatMessage_chatMessage"]',    // Robust message check
        '[data-message-id]',                     // Very robust message check
        'div[id^="message-"]'                    // Robust ID check
    ];

    const hasValidElement = selectors.some(selector => !!document.querySelector(selector));
    
    // If none of the key chat elements exist, assume it's not a chat page (e.g. 403, 404, or Home)
    if (!hasValidElement) {
       return { ok: 'false', error: 'INVALID_PAGE', message: "This page doesn't appear to be a Poe chat" };
    }
    return { ok: 'true' };
   }
   if (req && req.command === 'showError') {
     const args = req.args || {};
     updateStatus({ type: 'error', message: args.message, autoHideMs: 5000 });
     return { ok: 'true' };
   }
   if (req && req.command === 'scrollGetMetrics') {
    const m = getScrollMetrics();
    return {
      scrollTop: String(m.scrollTop),
      scrollHeight: String(m.scrollHeight),
      clientHeight: String(m.clientHeight),
      atTop: String(m.atTop),
      documentHidden: String(m.documentHidden),
      acceptsNegative: String(m.acceptsNegative),
      windowScrollX: String(m.windowScrollX),
      windowScrollY: String(m.windowScrollY)
    };
  }
  if (req && req.command === 'scrollSet') {
    const args = req.args || {};
    const value = typeof args.value === 'number' ? args.value : null;
    const container = findScrollContainer();
    if (value === null) return { ok: 'false', error: 'NO_VALUE' };
    container.scrollTop = value;
    container.dispatchEvent(new Event('scroll', { bubbles: true }));
    return { ok: 'true', appliedTop: String(container.scrollTop) };
  }
  if (req && req.command === 'scrollBy') {
    const args = req.args || {};
    const delta = typeof args.delta === 'number' ? args.delta : 0;
    const container = findScrollContainer();
    container.scrollTop = container.scrollTop + delta;
    container.dispatchEvent(new Event('scroll', { bubbles: true }));
    return { ok: 'true', appliedTop: String(container.scrollTop) };
  }
  if (req && req.command === 'domQuery') {
    const sel = (req.args && req.args.selector) || '';
    const inContainer = !!(req.args && req.args.inContainer);
    if (!sel) return { count: 0 };
    const root = inContainer ? findScrollContainer() : document;
    const nodes = root.querySelectorAll(sel);
    return { count: nodes.length };
  }
  if (req && req.command === 'domClick') {
    const sel = (req.args && req.args.selector) || '';
    const inContainer = !!(req.args && req.args.inContainer);
    if (!sel) return { clicked: 0 };
    const root = inContainer ? findScrollContainer() : document;
    const node = root.querySelector(sel);
    if (node) { try { node.click(); } catch (e) {} return { clicked: 1 }; }
    return { clicked: 0 };
  }
  if (req && req.command === 'startKeepAlive') {
    connectKeepAlivePort();
    return { ok: true };
  }
  if (req && req.command === 'stopKeepAlive') {
    stopKeepAlive();
    return { ok: true };
  }
  if (req && req.command === 'stopScroll') {
    window.isScrollCancelled = true;
    return { ok: true, status: 'stopping' };
  }
  if (req && req.command === 'windowSet') {
    const args = req.args || {};
    const xVal = (typeof args.x === 'number') ? args.x : (typeof args.windowX === 'number' ? args.windowX : window.scrollX);
    const yVal = (typeof args.y === 'number') ? args.y : (typeof args.windowY === 'number' ? args.windowY : window.scrollY);
    window.scrollTo(xVal, yVal);
    return { ok: 'true', windowScrollX: String(window.scrollX), windowScrollY: String(window.scrollY) };
  }
  if (req && req.command === 'getChatTitle') {
    try {
      const title = document.title || '';
      return { ok: 'true', title: String(title) };
    } catch (e) {
      return { ok: 'false', error: 'TITLE_FETCH_ERROR' };
    }
  }
  if (req && req.command === 'enumerateChats') {
    return enumerateChats();
  }
  return { ok: 'false' }
}

function rvSleep(ms) { return new Promise((r) => setTimeout(r, ms)); }

// Enumerate every conversation on the chats history page (https://poe.com/chats).
//
// IMPORTANT: Poe's history rows are NOT links — each row is
//   <li class="...ChatHistoryListItem_wrapper..."><div role="link">...</div></li>
// with no href; the chat's code lives in React's internal props. So we (1) scroll
// the infinite-scroll list to load every row (nudging the InfiniteScroll paging
// trigger), collecting as we go because rows recycle, and (2) derive each chat's
// URL from the row's React fiber props (with an <a href="/chat/..."> fallback).
// Returns { chatsJson: "[{url,title},...]" }.
//
// Poe-DOM-dependent (2026-06 capture) — tune selectors/field names here if Poe
// changes. If this finds 0 chats, the React-props extraction failed and we
// should fall back to click-to-navigate (see docs/TODO.md).
const SEL_HISTORY_ITEM = '[class*="ChatHistoryListItem_wrapper"]';
const SEL_HISTORY_TITLE = '[class*="ChatHistoryListItem_title"]';
const SEL_HISTORY_SCROLLER = '[class*="InfiniteScroll_container"]';
const SEL_HISTORY_TRIGGER = '[class*="InfiniteScroll_pagingTrigger"]';

function rvReactFiber(el) {
  const k = Object.keys(el).find(
    (k) => k.startsWith('__reactFiber$') || k.startsWith('__reactInternalInstance$')
  );
  return k ? el[k] : null;
}

// Look for a chat-code-like string in a props object (bounded recursion).
// Matches by KEY NAME pattern (chatCode/chatId/conversationId/code/…) with a
// value that looks like a Poe chat code (>=8 url-safe chars), rather than a
// fixed field list — more robust to Poe's prop shape.
function rvFindChatCode(obj, depth) {
  if (!obj || typeof obj !== 'object' || depth > 5) return null;
  for (const k in obj) {
    let v;
    try {
      v = obj[k];
    } catch (e) {
      continue;
    }
    if (typeof v === 'string' && /^[A-Za-z0-9_-]{8,}$/.test(v)) {
      const key = k.replace(/_/g, '').toLowerCase();
      if (/(chatcode|chatid|conversationid|^code$|^chatcode$)/.test(key)) return v;
    } else if (v && typeof v === 'object') {
      const r = rvFindChatCode(v, depth + 1);
      if (r) return r;
    }
  }
  return null;
}

function rvChatCodeOf(item) {
  // Fallback: a real anchor, if Poe ever renders one.
  const a = item.querySelector('a[href*="/chat/"]');
  if (a) {
    const m = (a.getAttribute('href') || '').match(/\/chat\/([A-Za-z0-9_-]+)/);
    if (m) return m[1];
  }
  // Primary: walk the row's React fiber chain for the chat code.
  let fiber = rvReactFiber(item);
  let depth = 0;
  while (fiber && depth < 40) {
    const code = rvFindChatCode(fiber.memoizedProps, 0);
    if (code) return code;
    fiber = fiber.return;
    depth++;
  }
  return null;
}

async function enumerateChats() {
  const seen = new Map(); // url -> title
  let itemsSeen = 0;

  const collect = () => {
    const items = document.querySelectorAll(SEL_HISTORY_ITEM);
    itemsSeen = Math.max(itemsSeen, items.length);
    items.forEach((item) => {
      const titleEl = item.querySelector(SEL_HISTORY_TITLE);
      const title = titleEl ? titleEl.textContent.replace(/\s+/g, ' ').trim().slice(0, 200) : '';
      const code = rvChatCodeOf(item);
      if (!code) return;
      const url = 'https://poe.com/chat/' + code;
      if (!seen.has(url) || (!seen.get(url) && title)) seen.set(url, title);
    });
  };

  const getScroller = () => {
    const c = document.querySelector(SEL_HISTORY_SCROLLER);
    if (c && c.scrollHeight > c.clientHeight + 20) return c;
    return findScrollContainer() || document.scrollingElement || document.documentElement;
  };

  // Wait for the history list to render before scrolling (the page may still be
  // hydrating right after navigation) — up to ~15s.
  for (let w = 0; w < 30; w++) {
    if (document.querySelectorAll(SEL_HISTORY_ITEM).length > 0) break;
    await rvSleep(500);
  }

  collect();
  let lastCount = seen.size;
  let stable = 0;
  for (let i = 0; i < 4000 && stable < 6; i++) {
    const sc = getScroller();
    const before = sc.scrollTop;
    // Pull the paging sentinel into view to trigger loading the next page.
    const trig = document.querySelector(SEL_HISTORY_TRIGGER);
    if (trig && trig.scrollIntoView) trig.scrollIntoView({ block: 'end' });
    sc.scrollTop = before + Math.max(400, (sc.clientHeight || 600) * 0.9);
    if (sc === document.scrollingElement || sc === document.documentElement) {
      window.scrollBy(0, 800);
    }
    await rvSleep(1000); // give the next page time to fetch + render

    collect();
    const moved = Math.abs(getScroller().scrollTop - before) > 2;
    const grew = seen.size !== lastCount;
    if (grew) lastCount = seen.size;
    if (!moved && !grew) stable++;
    else stable = 0;
  }

  const list = Array.from(seen, ([url, title]) => ({ url, title }));
  // Surface how many rows we saw vs URLs we resolved — diagnoses extraction.
  return { chatsJson: JSON.stringify(list), itemsSeen: String(itemsSeen) };
}


function safeSendMessage(msg, cb) {
  try {
    if (!(chrome && chrome.runtime && chrome.runtime.id)) return null;
    const res = (typeof cb === 'function')
      ? chrome.runtime.sendMessage(msg, cb)
      : chrome.runtime.sendMessage(msg);
    if (res && typeof res.then === 'function') res.catch(() => {});
    return res;
  } catch (e) {}
}

// Function to find the main scrollable container
function findScrollContainer() {
  // 1. Try specific Poe selector first (most robust)
  const poeContainer = document.querySelector('.ChatMessagesScrollWrapper_scrollableContainerWrapper__x8H60');
  if (poeContainer) return poeContainer;

  // 2. Fallback: look for largest scrollable element
  const allElements = document.querySelectorAll('*');
  let bestCandidate = null;
  let maxScrollHeight = 0;

  for (const el of allElements) {
    const style = window.getComputedStyle(el);
    if ((style.overflowY === 'scroll' || style.overflowY === 'auto') && el.scrollHeight > el.clientHeight) {
      if (el.scrollHeight > maxScrollHeight) {
        maxScrollHeight = el.scrollHeight;
        bestCandidate = el;
      }
    }
  }
  
  return bestCandidate || document.documentElement;
}

function containerAcceptsNegative(container) {
  const originalTop = container.scrollTop;
  container.scrollTop = -1;
  const acceptsNegative = container.scrollTop < 0;
  container.scrollTop = originalTop;
  return acceptsNegative;
}

function getScrollMetrics() {
  const container = findScrollContainer();
  const acceptsNegative = containerAcceptsNegative(container);
  const targetTop = acceptsNegative ? (container.clientHeight - container.scrollHeight) : 0;
  const atTop = Math.abs(container.scrollTop - targetTop) < 2;
  return {
    scrollTop: container.scrollTop,
    scrollHeight: container.scrollHeight,
    clientHeight: container.clientHeight,
    atTop,
    documentHidden: document.hidden === true,
    acceptsNegative,
    windowScrollX: window.scrollX,
    windowScrollY: window.scrollY
  };
}


// Session Validation on Focus
function checkSessionStatus() {
    try {
        readTabState();
        safeSendMessage({ type: 'GET_SESSION_STATUS' }, (response) => {
            if (chrome.runtime.lastError) {
                console.error('[Poe Scroll] checkSessionStatus error:', chrome.runtime.lastError);
                return;
            }
            
            // If local scroll has been cancelled, treat as aborted immediately
            if (window.isScrollCancelled) {
                if (!window.abortedHandled) {
                    window.isScrolling = false;
                    hideScrollGuard();
                    updateStatus({
                        type: 'aborted',
                        message: UI_CONSTANTS.MESSAGES.EXPORT_ABORTED_TIMEOUT
                    });
                    stopKeepAlive();
                    window.abortedHandled = true;
                    writeTabState({ abortedHandled: true, wasExporting: false });
                }
                return;
            }

            if (!response || !response.active) {
                if (window.isScrolling) {
                    window.isScrollCancelled = true;
                    window.isScrolling = false;
                    hideScrollGuard();
                    updateStatus({ 
                        type: 'aborted', 
                        message: UI_CONSTANTS.MESSAGES.CONNECTION_LOST_INACTIVE, 
                        autoHideMs: 12000 
                    });
                    stopKeepAlive();
                } else if (window.wasExportingInThisTab && !window.abortedHandled) {
                    window.isScrollCancelled = true;
                    window.isScrolling = false;
                    hideScrollGuard();
                    updateStatus({
                        type: 'aborted',
                        message: UI_CONSTANTS.MESSAGES.EXPORT_ABORTED_TIMEOUT
                    });
                    stopKeepAlive();
                    window.abortedHandled = true;
                    writeTabState({ abortedHandled: true, wasExporting: false });
                }
            } else {
                // Session valid. Continuing.
            }
        });
    } catch (e) {
        // Runtime might be invalid if extension reloaded
        console.error('Failed to check session status:', e);
    }
}

document.addEventListener('visibilitychange', () => {
    // Send visibility status to background -> host
    safeSendMessage({ 
        type: 'VISIBILITY_CHANGED', 
        hidden: document.hidden 
    });

    if (!document.hidden) {
        checkSessionStatus();
    }
});



if (typeof window.isScrolling === 'undefined') window.isScrolling = false;
if (typeof window.isScrollCancelled === 'undefined') window.isScrollCancelled = false;
if (typeof window.wasExportingInThisTab === 'undefined') window.wasExportingInThisTab = false;
if (typeof window.abortedHandled === 'undefined') window.abortedHandled = false;
function readTabState() {
  const w = sessionStorage.getItem('rv_wasExporting') === '1';
  const a = sessionStorage.getItem('rv_abortedHandled') === '1';
  window.wasExportingInThisTab = w;
  window.abortedHandled = a;
}
function writeTabState(updates) {
  if (updates && typeof updates.wasExporting === 'boolean') {
    sessionStorage.setItem('rv_wasExporting', updates.wasExporting ? '1' : '0');
    window.wasExportingInThisTab = updates.wasExporting;
  }
  if (updates && typeof updates.abortedHandled === 'boolean') {
    sessionStorage.setItem('rv_abortedHandled', updates.abortedHandled ? '1' : '0');
    window.abortedHandled = updates.abortedHandled;
  }
}
readTabState();

let keepAlivePort = null;
let keepAliveRotationInterval = null;
function stopKeepAlive() {
  if (keepAliveRotationInterval) {
    clearInterval(keepAliveRotationInterval);
    keepAliveRotationInterval = null;
  }
  if (keepAlivePort) {
    try { keepAlivePort.disconnect(); } catch (e) {}
    keepAlivePort = null;
  }
}

function connectKeepAlivePort() {
  if (keepAlivePort) return keepAlivePort;

  try {
    keepAlivePort = chrome.runtime.connect({ name: 'keepAlive' });
    
    // Auto-rotate every 25s to prevent Service Worker idle timeout (29s rule)
    if (keepAliveRotationInterval) clearInterval(keepAliveRotationInterval);
    keepAliveRotationInterval = setInterval(() => {
        if (keepAlivePort) {
            try { 
                // Remove listener to prevent immediate reconnect trigger
                // (Actually onDisconnect is not fired for local disconnect, but just in case)
                keepAlivePort.disconnect(); 
            } catch(e) {}
            keepAlivePort = null;
        }
        connectKeepAlivePort();
    }, 25000);

    keepAlivePort.onDisconnect.addListener(() => {
      const lastError = chrome.runtime.lastError;
      console.warn('[Poe Scroll] KeepAlive Port disconnected unexpectedly.', lastError ? lastError.message : '');
      keepAlivePort = null;
      // Immediate Reconnect
      setTimeout(connectKeepAlivePort, 1000);
    });
    
    // Initial Ping to wake up SW
    keepAlivePort.postMessage({ type: 'PING' });

  } catch (e) {
    console.error('[Poe Scroll] Failed to create KeepAlive port', e);
  }
  return keepAlivePort;
}

 



chrome.runtime.onMessage.addListener((req, sender, sendResponse) => {
  if (req.type === 'UI_RENDER') {
      const ui = req.ui;
      if (!ui) { sendResponse({ ok: false }); return true; }
      
      switch (ui.type) {
          case 'progress':
              updateStatus({
                  type: 'progress',
                  current: ui.current,
                  total: ui.total,
                  message: ui.message
              });
              break;
          case 'success':
               window.abortedHandled = true;
               window.wasExportingInThisTab = false;
               writeTabState({ abortedHandled: true, wasExporting: false });
               updateStatus({
                  type: 'success',
                  message: ui.message || UI_CONSTANTS.MESSAGES.EXPORT_COMPLETE,
                  autoHideMs: ui.autoHideMs,
                  showOpenButton: ui.showOpenButton
               });
               break;
          case 'error':
              updateStatus({
                  type: 'error',
                  message: ui.message,
                  autoHideMs: ui.autoHideMs
              });
              break;
          case 'hide':
              hideScrollGuard();
              if (statusEl) { statusEl.remove(); statusEl = null; }
              break;
      }
      sendResponse({ ok: true });
      return true;
  }

  if (req.type === 'UPDATE_STATUS') {
    if (req.options && req.options.type === 'success') {
        window.abortedHandled = true;
        window.wasExportingInThisTab = false;
        writeTabState({ abortedHandled: true, wasExporting: false });
    }
    updateStatus(req.options);
    sendResponse({ ok: true });
    return;
  }
  if (req.type === 'HIDE_PROGRESS') {
    hideScrollGuard();
    if (statusEl) { statusEl.remove(); statusEl = null; }
    sendResponse({ ok: true });
    return;
  }
  
  Promise.resolve(handle(req)).then(sendResponse)
  return true
})

// UI Helpers
let statusEl = null;
let styleEl = null;

function ensureStyles() {
  if (styleEl && document.head.contains(styleEl)) return;
  styleEl = document.createElement('style');
  styleEl.textContent = `
    :root {
      /* COLORS - Core Palette */
      --rv-primary-400: #a78bfa;
      --rv-primary-500: #8b5cf6;
      --rv-primary-600: #7c3aed;
      
      --rv-success-400: #4ade80;
      --rv-success-500: #22c55e;
      
      --rv-error-400: #f87171;
      --rv-error-600: #dc2626;
      
      --rv-slate-300: #cbd5e1;
      --rv-slate-400: #94a3b8;
      --rv-slate-500: #64748b;
      --rv-slate-600: #475569;
      --rv-slate-700: #334155;
      --rv-slate-800: #1e293b;
      --rv-slate-900: #0f172a;
      
      /* SEMANTIC COLORS */
      --rv-bg-overlay-backdrop: rgba(15, 23, 42, 0.85);
      --rv-bg-card: var(--rv-slate-800);
      --rv-bg-card-translucent: rgba(30, 41, 59, 0.95);
      --rv-bg-button-primary: var(--rv-primary-600);
      --rv-bg-button-primary-hover: var(--rv-primary-500);
      --rv-bg-button-secondary: var(--rv-slate-700);
      --rv-bg-button-secondary-hover: var(--rv-slate-600);
      --rv-bg-success: var(--rv-success-500);
      --rv-bg-error: var(--rv-error-600);
      --rv-bg-cancelled: var(--rv-slate-700);
      
      --rv-bg-icon-primary: rgba(139, 92, 246, 0.2);
      --rv-bg-icon-success: rgba(34, 197, 94, 0.2);
      
      --rv-border-default: var(--rv-slate-700);
      --rv-border-subtle: rgba(51, 65, 85, 0.5);
      
      --rv-text-primary: #ffffff;
      --rv-text-secondary: var(--rv-slate-400);
      --rv-text-tertiary: var(--rv-slate-500);
      
      /* DIMENSIONS */
      --rv-radius-md: 8px;
      --rv-radius-lg: 12px;
      --rv-radius-full: 9999px;
      
      /* SHADOWS */
      --rv-shadow-lg: 0 10px 15px -3px rgba(0, 0, 0, 0.1);
      --rv-shadow-2xl: 0 25px 50px -12px rgba(0, 0, 0, 0.25);
      
      /* ANIMATIONS */
      --rv-transition-normal: 200ms ease;
    }
    
    /* Common Reset */
    .poe-scroll-guard-overlay *, .poe-status-pill * {
      box-sizing: border-box;
      font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
    }

    /* --- Scroll Guard Overlay --- */
    .poe-scroll-guard-overlay {
      position: fixed; top: 0; left: 0; right: 0; bottom: 0;
      background: var(--rv-bg-overlay-backdrop);
      backdrop-filter: blur(4px);
      z-index: 2147483647;
      display: flex; align-items: center; justify-content: center;
      touch-action: none;
      overscroll-behavior: none;
    }
    
    .poe-scroll-guard-card {
      width: 320px;
      background: var(--rv-bg-card);
      border: 1px solid var(--rv-border-default);
      border-radius: var(--rv-radius-lg);
      padding: 24px;
      box-shadow: var(--rv-shadow-2xl);
      display: flex; flex-direction: column; align-items: center;
    }

    .poe-guard-reminder {
      display: flex; align-items: center; justify-content: center; gap: 8px;
      margin-bottom: 24px;
      padding: 8px 12px;
      background: rgba(51, 65, 85, 0.5);
      border-radius: var(--rv-radius-md);
      font-size: 13px;
      font-weight: 500;
      color: var(--rv-slate-300);
      width: 100%;
      border: 1px solid rgba(255, 255, 255, 0.05);
    }

    .poe-guard-reminder svg {
      color: var(--rv-slate-400);
      flex-shrink: 0;
    }

    .poe-icon-container {
      width: 48px; height: 48px;
      background: var(--rv-bg-icon-primary);
      border-radius: var(--rv-radius-lg);
      display: flex; align-items: center; justify-content: center;
      margin-bottom: 16px;
    }

    .poe-icon-arrow {
      color: var(--rv-primary-400);
      font-size: 24px;
      animation: arrow-bounce 600ms infinite;
    }

    .poe-guard-title {
      font-size: 18px; font-weight: 600; color: var(--rv-text-primary);
      margin-bottom: 4px; text-align: center;
    }

    .poe-guard-subtext {
      font-size: 14px; color: var(--rv-text-secondary);
      margin-bottom: 16px; text-align: center;
    }

    .poe-activity-indicator {
      display: flex; align-items: center; gap: 4px;
      margin-bottom: 16px;
    }

    .poe-dot {
      width: 6px; height: 6px; border-radius: 50%;
      background: var(--rv-primary-500);
      animation: dots-pulse 1.5s infinite;
    }
    .poe-dot:nth-child(2) { animation-delay: 0.2s; }
    .poe-dot:nth-child(3) { animation-delay: 0.4s; }

    .poe-message-count {
      font-size: 14px; color: var(--rv-text-secondary);
      margin-left: 8px;
    }

    .poe-btn-cancel {
      width: 100%;
      padding: 10px 16px;
      background: var(--rv-bg-button-secondary);
      border-radius: var(--rv-radius-md);
      font-size: 14px; font-weight: 500; color: var(--rv-slate-300);
      border: none; cursor: pointer;
      transition: var(--rv-transition-normal);
    }
    .poe-btn-cancel:hover { background: var(--rv-bg-button-secondary-hover); }

    /* --- Status Pills --- */
    .poe-status-pill {
      position: fixed; top: 16px; right: 24px;
      z-index: 2147483647;
      box-shadow: var(--rv-shadow-lg);
      font-size: 14px;
      animation: fade-in 200ms ease-out;
    }

    /* Downloading State */
    .poe-status-downloading {
      background: var(--rv-bg-card-translucent);
      backdrop-filter: blur(4px);
      border: 1px solid var(--rv-border-subtle);
      border-radius: var(--rv-radius-lg);
      overflow: hidden;
      min-width: 280px;
    }

    .poe-status-downloading .poe-pill-content {
      padding: 12px 16px;
      display: flex; align-items: center; gap: 12px;
    }

    .poe-spinner {
      width: 16px; height: 16px;
      border: 2px solid rgba(167, 139, 250, 0.3);
      border-top-color: var(--rv-primary-400);
      border-radius: 50%;
      animation: spinner-rotate 1s linear infinite;
    }

    .poe-pill-text-white { color: var(--rv-text-primary); font-weight: 500; }
    .poe-pill-sep { color: var(--rv-slate-500); margin: 0 4px; }

    .poe-progress-track {
      height: 4px; background: var(--rv-slate-700);
      width: 100%;
    }
    .poe-progress-fill {
      height: 100%; background: var(--rv-primary-500);
      transition: width 0.25s ease;
    }

    /* Success State */
    .poe-status-success {
      background: var(--rv-bg-card-translucent);
      backdrop-filter: blur(4px);
      border: 1px solid var(--rv-border-subtle);
      border-radius: var(--rv-radius-lg);
      overflow: hidden;
      min-width: 300px;
    }

    .poe-accent-bar {
      height: 4px;
      background: linear-gradient(to right, #22c55e, #10b981);
    }

    .poe-pill-content-col { padding: 16px; }
    .poe-pill-row { display: flex; align-items: center; gap: 12px; margin-bottom: 12px; }

    .poe-icon-circle-success {
      width: 32px; height: 32px;
      background: var(--rv-bg-icon-success);
      border-radius: 50%;
      display: flex; align-items: center; justify-content: center;
      color: var(--rv-success-400);
      font-size: 16px;
    }

    .poe-pill-title { color: var(--rv-text-primary); font-weight: 500; font-size: 14px; }

    .poe-pill-actions { display: flex; gap: 8px; }

    .poe-btn {
      flex: 1; padding: 8px 16px; border-radius: var(--rv-radius-md);
      font-size: 14px; font-weight: 500; border: none; cursor: pointer;
      display: flex; align-items: center; justify-content: center; gap: 6px;
      transition: var(--rv-transition-normal);
    }
    
    .poe-btn-primary {
      background: var(--rv-bg-button-primary); color: #ffffff;
    }
    .poe-btn-primary:hover { background: var(--rv-bg-button-primary-hover); }

    .poe-btn-secondary {
      background: var(--rv-bg-button-secondary); color: var(--rv-slate-300);
    }
    .poe-btn-secondary:hover { background: var(--rv-bg-button-secondary-hover); }

    /* Error / Cancelled State */
    .poe-status-error {
      background: var(--rv-bg-error);
      border-radius: var(--rv-radius-full);
      padding: 8px 16px;
      display: flex; align-items: center; gap: 8px;
      color: #ffffff;
    }
    
    .poe-status-cancelled {
      background: var(--rv-bg-cancelled);
      border: 1px solid var(--rv-slate-600);
      border-radius: var(--rv-radius-full);
      padding: 8px 16px;
      display: flex; align-items: center; gap: 8px;
      color: var(--rv-slate-300);
    }

    /* Animations */
    @keyframes arrow-bounce {
      0%, 100% { transform: translateY(0); }
      50% { transform: translateY(-3px); }
    }
    @keyframes dots-pulse {
      0%, 100% { opacity: 0.3; }
      50% { opacity: 1; }
    }
    @keyframes spinner-rotate {
      from { transform: rotate(0deg); }
      to { transform: rotate(360deg); }
    }
    @keyframes fade-in {
      from { opacity: 0; transform: translateY(-8px); }
      to { opacity: 1; transform: translateY(0); }
    }
    @keyframes fade-out {
      from { opacity: 1; }
      to { opacity: 0; }
    }
    .poe-fade-out {
      animation: fade-out 200ms ease-out forwards;
    }
  `;
  document.head.appendChild(styleEl);
}

// Scroll Guard State
let guardEl = null;

function showScrollGuard() {
  if (guardEl) return;
  
  ensureStyles();
  
  guardEl = document.createElement('div');
  guardEl.className = 'poe-scroll-guard-overlay';
  guardEl.innerHTML = `
    <div class="poe-scroll-guard-card" role="dialog" aria-modal="true" aria-labelledby="poe-guard-title" aria-live="polite">
      <div class="poe-guard-reminder">
        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
          <path d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z"></path>
          <circle cx="12" cy="12" r="3"></circle>
        </svg>
        <span>${UI_CONSTANTS.OVERLAY.KEEP_TAB_OPEN}</span>
      </div>
      <div class="poe-icon-container">
        <div class="poe-icon-arrow">
             <svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
                <line x1="12" y1="19" x2="12" y2="5"></line>
                <polyline points="5 12 12 5 19 12"></polyline>
             </svg>
        </div>
      </div>
      <h2 id="poe-guard-title" class="poe-guard-title">${UI_CONSTANTS.OVERLAY.TITLE}</h2>
      <p class="poe-guard-subtext">${UI_CONSTANTS.OVERLAY.SUBTEXT}</p>
      
      <div class="poe-activity-indicator">
        <div class="poe-dot"></div><div class="poe-dot"></div><div class="poe-dot"></div>
        <span class="poe-message-count" id="poe-msg-count"></span>
      </div>

      <button class="poe-btn-cancel" id="poe-guard-cancel">${UI_CONSTANTS.OVERLAY.BUTTON_CANCEL}</button>
    </div>
  `;
  
  document.body.appendChild(guardEl);
  
  // Trap events to prevent scroll
  const prevent = (e) => { e.preventDefault(); e.stopPropagation(); };
  guardEl.addEventListener('wheel', prevent, { passive: false });
  guardEl.addEventListener('touchmove', prevent, { passive: false });
  
  // Cancel handler
  const btn = guardEl.querySelector('#poe-guard-cancel');
  if (btn) {
      btn.onclick = () => {
        isScrollCancelled = true;
        safeSendMessage({ type: 'CANCEL_EXPORT' });
      };
  }
}

function hideScrollGuard() {
  if (!guardEl) return;
  
  guardEl.remove();
  guardEl = null;
}

function updateStatus(opts) {
  // opts: { type, message, current, total, autoHideMs }
  ensureStyles();
  
  const type = opts.type || 'progress';
  let message = opts.message || '';
  const isScroll = message && /\b(scrolling|loading|preparing|scanning)\b/i.test(message);
  
  // Manage Scroll Guard
  if (type === 'progress' && isScroll) {
      showScrollGuard();
      if (guardEl && opts.current) {
          const countEl = guardEl.querySelector('#poe-msg-count');
          if (countEl) countEl.textContent = `${opts.current} messages`;
      }
      // Hide pill if it exists (it's behind overlay anyway)
      if (statusEl) { statusEl.remove(); statusEl = null; }
      return;
  } else {
      hideScrollGuard();
  }

  // Determine logic variables
  let pillClass = '';
  let htmlContent = '';
  let autoHide = opts.autoHideMs;

  if (type === 'progress') {
      // Downloading
      pillClass = 'poe-status-pill poe-status-downloading';
      const hasCounts = Number.isFinite(opts.current) && Number.isFinite(opts.total) && opts.total > 0;
      const pct = hasCounts ? Math.round((opts.current / opts.total) * 100) : 0;
      const text = hasCounts 
        ? `Downloading files <span class="poe-pill-sep">·</span> ${opts.current} of ${opts.total}`
        : `Downloading files`;
      
      htmlContent = `
        <div class="poe-pill-content">
          <div class="poe-spinner"></div>
          <span class="poe-pill-text poe-pill-text-white">${text}</span>
        </div>
        <div class="poe-progress-track">
          <div class="poe-progress-fill" style="width: ${pct}%"></div>
        </div>
      `;
  } else if (type === 'success') {
      pillClass = 'poe-status-pill poe-status-success';
      
      htmlContent = `
        <div class="poe-accent-bar"></div>
        <div class="poe-pill-content-col">
          <div class="poe-pill-row">
            <div class="poe-icon-circle-success">✓</div>
            <div>
              <div class="poe-pill-title">${message || UI_CONSTANTS.MESSAGES.EXPORT_COMPLETE}</div>
            </div>
          </div>
          <div class="poe-pill-actions">
             ${opts.showOpenButton ? `<button class="poe-btn poe-btn-primary" id="poe-btn-open">Open ↗</button>` : ''}
             <button class="poe-btn poe-btn-secondary" id="poe-btn-dismiss">Dismiss</button>
          </div>
        </div>
      `;
      autoHide = 0; // Success manual dismiss only
      
      // Update state
      window.abortedHandled = true;
      window.wasExportingInThisTab = false;
      writeTabState({ abortedHandled: true, wasExporting: false });

  } else if (type === 'error') {
      pillClass = 'poe-status-pill poe-status-error';
      // Use provided message or default
      if (!message) message = "Export failed";
      
      htmlContent = `
        <div class="poe-icon-sm" style="font-size:16px;">✕</div>
        <div class="poe-pill-text" style="font-weight:500;">${message}</div>
      `;
      // Auto-dismiss after 4s (default usually passed in opts, but enforce if missing)
      if (!autoHide) autoHide = 4000;
      
  } else if (type === 'aborted') {
      pillClass = 'poe-status-pill poe-status-cancelled';
      if (!message) message = "Export cancelled";
      
      // Ensure red/grey background handled by class
      isScrollCancelled = true;
      hideScrollGuard();
      stopKeepAlive();
      window.abortedHandled = true;
      window.wasExportingInThisTab = false;
      writeTabState({ abortedHandled: true, wasExporting: false });
      
      htmlContent = `
        <div class="poe-icon-sm" style="font-size:16px;">✕</div>
        <div class="poe-pill-text" style="font-weight:500;">${message}</div>
      `;
      if (!autoHide) autoHide = 4000;
  } else if (type === 'warning') {
     // Fallback for warning if needed
     pillClass = 'poe-status-pill poe-status-error';
      htmlContent = `
        <div class="poe-pill-text">${message}</div>
      `;
  }

  // Create or Update Element
  if (!statusEl) {
      statusEl = document.createElement('div');
      document.body.appendChild(statusEl);
  }
  
  statusEl.className = pillClass;
  statusEl.innerHTML = htmlContent;

  // Event Handlers
  if (type === 'success') {
      const btnOpen = statusEl.querySelector('#poe-btn-open');
      const btnDismiss = statusEl.querySelector('#poe-btn-dismiss');

      const dismiss = () => {
          document.removeEventListener('click', dismiss);
          if (statusEl) {
              statusEl.remove();
              statusEl = null;
          }
      };

      // Add global click listener for "click anywhere" (inside or outside)
      // Delay slightly to ensure we don't catch any immediate events
      setTimeout(() => {
          document.addEventListener('click', dismiss);
      }, 100);

      if (btnOpen) btnOpen.onclick = () => {
          safeSendMessage({ type: 'OPEN_RESULT' });
          dismiss();
      };
      
      if (btnDismiss) btnDismiss.onclick = () => {
          dismiss();
      };
  } 

  // Auto Hide Logic
  if (statusEl._hideTimer) clearTimeout(statusEl._hideTimer);
  
  if (autoHide) {
        if (document.hidden && (type === 'aborted' || type === 'error')) {
        // If hidden and it's an error/abort, defer auto-hide until visible
        const onVisible = () => {
            if (!document.hidden) {
                document.removeEventListener('visibilitychange', onVisible);
                statusEl._hideTimer = setTimeout(() => {
                    if (statusEl) {
                        statusEl.classList.add('poe-fade-out');
                        setTimeout(() => {
                             if (statusEl) { statusEl.remove(); statusEl = null; }
                        }, 200);
                    }
                }, autoHide);
            }
        };
        document.addEventListener('visibilitychange', onVisible);
    } else {
        statusEl._hideTimer = setTimeout(() => {
            if (statusEl) {
                statusEl.classList.add('poe-fade-out');
                setTimeout(() => {
                     if (statusEl) { statusEl.remove(); statusEl = null; }
                }, 200);
            }
        }, autoHide);
    }
  }
}
