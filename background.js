/*
 * MIT License
 * Copyright (c) 2026 RavenVault
 * See LICENSE file for details.
 */

importScripts('config.js', 'ui-constants.js');

// --- State for Progress Tracking ---
// We enforce a SINGLE export session at a time.
// Session: {
//   sessionId: string,
//   tabId: number,
//   windowId: number
// }
let currentSession = null;

let wsClient = null;
let connectPromise = null;
const responseListeners = new Map();
let lastSentUrl = null;
let lastSentAt = 0;
let inflightUrl = null;
let queuedForce = false;
let hasQueued = false;

let pendingAutoStart = null;
let launchPollingInterval = null;
let userCancelled = false;
let pendingSessionStatusCallbacks = {};
let lastConnectionAttempt = 0;
let versionCheckError = null; // Stores critical version incompatibility error

// Helper for Semantic Version Comparison
function compareVersions(v1, v2) {
    if (!v1 || !v2) return 0;
    const p1 = v1.split('.').map(Number);
    const p2 = v2.split('.').map(Number);
    const len = Math.max(p1.length, p2.length);
    for (let i = 0; i < len; i++) {
        const n1 = p1[i] || 0;
        const n2 = p2[i] || 0;
        if (n1 > n2) return 1;
        if (n1 < n2) return -1;
    }
    return 0;
}

function getTabIdFromMessage(msg) {
    return msg.args?.tabId || msg.args?.session?.tabId;
}

function shouldSend(url, force) {
  if (force) return true;
  if (url === lastSentUrl) {
    if ((Date.now() - lastSentAt) < 300) return false;
    return false;
  }
  if (url === '' && lastSentUrl === '' && (Date.now() - lastSentAt) < 1000) return false;
  return true;
}


async function clearSession(keepStatus = false, skipStopScroll = false) {
    if (currentSession) {
        const tid = currentSession.tabId;
        
        if (!skipStopScroll) {
            // Stop any scrolling
            try {
                await execContent(tid, { command: 'stopScroll' });
            } catch (e) {}
        }

        // Restore popup for this tab so next click opens it
        try {
            await chrome.action.setPopup({ tabId: tid, popup: 'popup.html' });
        } catch (e) {}

        currentSession = null;
        
        if (!keepStatus) {
            // Notify UI to hide status
            try {
                await execContent(tid, { type: 'HIDE_PROGRESS' });
            } catch (e) {}
        }
    }
}

 




async function connectWebSocket(timeoutMs = CONFIG.TIMEOUT_CONNECT_MS) {
   if (wsClient && wsClient.readyState === WebSocket.OPEN) return wsClient;
   
   if (connectPromise) return connectPromise;
  
    connectPromise = new Promise((resolve) => {
      // Timeout safety
      const timeoutId = setTimeout(() => {
          resolve({ error: UI_CONSTANTS.ERRORS.CONNECTION_TIMED_OUT });
          connectPromise = null;
      }, timeoutMs);
  
      const tryConnect = () => {
        let ws;
        try {
          ws = new WebSocket(CONFIG.WEBSOCKET_URL);
        } catch (e) {
          clearTimeout(timeoutId);
          resolve({ error: e.message });
          connectPromise = null;
          return;
        }
        ws.onopen = async () => {
          wsClient = ws;
          clearTimeout(timeoutId);
          
          // Send Handshake with Version
          try {
              const manifest = chrome.runtime.getManifest();
              ws.send(JSON.stringify({
                  version: '1',
                  request_id: crypto.randomUUID(),
                  source: 'extension',
                  type: 'command',
                  command: 'handshake',
                  args: { version: manifest.version }
              }));
          } catch (e) {}
          
          resolve(ws);
          connectPromise = null;
          logCurrentActiveTabURL(true);
        };
        ws.onmessage = (event) => {
          try {
            const msg = JSON.parse(event.data);
            onHostMessage(msg);
          } catch (e) {
          }
        };
        ws.onerror = () => {
          if (connectPromise) {
            clearTimeout(timeoutId);
            resolve({ error: UI_CONSTANTS.ERRORS.CONNECTION_FAILED });
            connectPromise = null;
          }
        };
        ws.onclose = async () => {
          if (wsClient === ws) {
              wsClient = null;
          }
        if (connectPromise) {
          clearTimeout(timeoutId);
          resolve({ error: UI_CONSTANTS.ERRORS.CONNECTION_CLOSED });
          connectPromise = null;
        }
        
        // Snapshot session to avoid race conditions during async operations
        const session = currentSession;
        if (session) {
          const { tabId, windowId, sessionId } = session;
          
          try {
            await execContent(tabId, {
              type: 'UPDATE_STATUS',
              options: { type: 'aborted', message: UI_CONSTANTS.MESSAGES.EXPORT_ABORTED_TIMEOUT }
            });
          } catch (e) {}
          
          const vis = await getExportTabVisibility();
          
          // CRITICAL: Check if session is still active and unchanged before aborting/clearing
          if (currentSession && currentSession.sessionId === sessionId) {
              try {
                  const tRes = await execContent(tabId, { command: 'getChatTitle' });
                  const chatTitle = tRes && tRes.title ? String(tRes.title) : '';
                  await sendToHost({
                      version: '1', request_id: crypto.randomUUID(), source: 'extension', type: 'command',
                      command: 'abort_export', args: { message: UI_CONSTANTS.MESSAGES.TIMED_OUT_WAITING, chatTitle, isBackground: vis.isBackground, tabId: tabId, windowId: windowId }
                  });
              } catch (e) {
                  await sendToHost({
                      version: '1', request_id: crypto.randomUUID(), source: 'extension', type: 'command',
                      command: 'abort_export', args: { message: UI_CONSTANTS.MESSAGES.TIMED_OUT_WAITING, isBackground: vis.isBackground, tabId: tabId, windowId: windowId }
                  });
              }
              await clearSession(true);
          }
        }
      };
    };
    tryConnect();
  });
  
  return connectPromise;
}


async function onHostMessage(msg) {
    if (msg.request_id && responseListeners.has(msg.request_id)) {
        responseListeners.get(msg.request_id)(msg);
    }
    
    // Handshake Response: Version Validation
    if (msg.type === 'response' && msg.command === 'handshake') {
        const res = msg.result || {};
        const appVersion = res.app_version;
        const minExtVersion = res.min_extension_version;
        const manifest = chrome.runtime.getManifest();
        const myVersion = manifest.version;
        
        versionCheckError = null;
        
        // 1. Check if Extension is too old (App requirement)
        if (minExtVersion && compareVersions(myVersion, minExtVersion) < 0) {
            versionCheckError = `${UI_CONSTANTS.ERRORS.EXTENSION_OUTDATED} (v${myVersion} < v${minExtVersion})`;
            console.error(versionCheckError);
        }
        
        // 2. Check if App is too old (Extension requirement)
        // Hardcoded requirement: App must be at least 0.9.1 to support the current IPC protocol.
        // If the App is older, we block usage to prevent protocol mismatch errors.
        const MIN_APP_VERSION = CONFIG.MIN_APP_VERSION;
        if (appVersion && compareVersions(appVersion, MIN_APP_VERSION) < 0) {
            versionCheckError = `${UI_CONSTANTS.ERRORS.APP_OUTDATED} (v${appVersion} < v${MIN_APP_VERSION})`;
            console.error(versionCheckError);
        }
    }
    
    if (msg.command === 'session_status' && msg.type === 'response') {
        // Inject version error if present
        if (versionCheckError) {
            msg.result = msg.result || {};
            msg.result.error = versionCheckError;
            // Force inactive
            msg.result.active = "false";
        }
        
        const callback = pendingSessionStatusCallbacks[msg.request_id];
        if (callback) {
            delete pendingSessionStatusCallbacks[msg.request_id];
            callback(msg);
        }
        return;
    }

    // 0. Handle App Connection Event
    if (msg.type === 'event' && msg.command === 'app_connected') {
        logCurrentActiveTabURL(true);
    }

    // 0.5 Handle App-Driven Capture Start
    if (msg.command === 'capture_start') {
        (async () => {
            try {
                const sessionArgs = msg.args && msg.args.session;
                const tabId = getTabIdFromMessage(msg);
                
                if (tabId) {
                    // Fetch Title
                    const tRes = await execContent(tabId, { command: 'getChatTitle' });
                    const title = tRes && tRes.title ? tRes.title : 'Export';
                    
                    // Update status (optional, but good for UX)
                    await execContent(tabId, { 
                        type: 'UPDATE_STATUS', 
                        options: { type: 'progress', message: UI_CONSTANTS.MESSAGES.PREPARING_EXPORT, current: 0, total: 0 } 
                    });
                    
                    // Prepare Session & ID
                    const rid = crypto.randomUUID();
                    
                    // Update session for progress tracking
                    if (currentSession && currentSession.tabId === tabId) {
                        // existing session, no status updates needed locally
                    } else {
                        currentSession = {
                            sessionId: crypto.randomUUID(),
                            tabId: tabId,
                            windowId: sessionArgs.windowId || 0
                        };
                        // session adopted from app context
                    }
                    
                    // Capture and Stream
                    const res = await sendPageHTML(tabId, title, false, 'capture_complete', rid);
                    
                    if (!res.ok) {
                        await logToHost(`Error: Failed to capture HTML (App Driven). Details: ${res.error}`);
                        // Should probably send error to app?
                        // But app has timeout logic.
                        await execContent(tabId, { 
                            type: 'UPDATE_STATUS', 
                            options: { type: 'error', message: `${UI_CONSTANTS.ERRORS.CAPTURE_HTML_FAILED}${res.error}`, autoHideMs: 5000 } 
                        });
                    }
                }
            } catch (e) {
                const errStr = e && e.message ? e.message : 'UNKNOWN_CAPTURE_ERROR';
                await logToHost(`Error: Exception in capture_start. Details: ${errStr}`);
                // Try to notify UI if possible
                const sessionArgs = msg.args && msg.args.session;
                const tabId = sessionArgs ? sessionArgs.tabId : null;
                if (tabId) {
                    try {
                        await execContent(tabId, { 
                           type: 'UPDATE_STATUS', 
                           options: { type: 'error', message: `${UI_CONSTANTS.ERRORS.CAPTURE_HTML_FAILED}${errStr}`, autoHideMs: 5000 } 
                        });
                    } catch (ex) {}
                }
            }
        })();
    }

 

  // 1.5 Handle Download Request from Host
  if (msg.type === 'request' && msg.command === 'download') {
    const url = msg.args?.url;
    if (url) {
      logToHost('Received download request for', url);
      handleHostDownloadRequest(url);
    } else {
      logToHost('Received download request but URL is missing');
    }
  }
  
  // 1.6 Bulk export: navigate the tab to a chat URL (app-driven loop).
  if (msg.type === 'request' && msg.command === 'navigate') {
    (async () => {
      try {
        if (!userCancelled && msg.args && msg.args.session) {
          const s = msg.args.session;
          if (!currentSession || currentSession.tabId !== s.tabId) {
            currentSession = { sessionId: `app-${s.tabId}`, tabId: s.tabId, windowId: s.windowId };
          }
        }
        const tabId = getTabIdFromMessage(msg);
        if (!tabId) throw new Error('NO_ACTIVE_SESSION');
        const url = msg.args && msg.args.url;
        if (!url) throw new Error('NO_URL');
        await navigateTabAndWait(tabId, url);
        await sendToHost({
          version: '1', request_id: msg.request_id, source: 'extension',
          type: 'response', command: 'navigate', result: { ok: 'true' }
        });
      } catch (e) {
        await sendToHost({
          version: '1', request_id: msg.request_id, source: 'extension',
          type: 'error', command: 'navigate', error: { message: (e && e.message) || 'NAVIGATE_ERROR' }
        });
      }
    })();
    return;
  }

  // 1.7 Bulk export: enumerate the full chat history. Navigates the tab to the
  // chats page, then has the content script scroll-scrape every /chat/ link.
  if (msg.type === 'request' && msg.command === 'list_chats') {
    (async () => {
      try {
        if (!userCancelled && msg.args && msg.args.session) {
          const s = msg.args.session;
          if (!currentSession || currentSession.tabId !== s.tabId) {
            currentSession = { sessionId: `app-${s.tabId}`, tabId: s.tabId, windowId: s.windowId };
          }
        }
        const tabId = getTabIdFromMessage(msg);
        if (!tabId) throw new Error('NO_ACTIVE_SESSION');
        await navigateTabAndWait(tabId, 'https://poe.com/chats');
        // Enumerate in the PAGE's MAIN world: React fibers (which hold the chat
        // code) are only visible there, not from the content script's isolated
        // world.
        let res = { list: [], itemsSeen: 0, sample: 'no-result' };
        try {
          const out = await chrome.scripting.executeScript({
            target: { tabId },
            world: 'MAIN',
            func: pageEnumerateChats
          });
          if (out && out[0] && out[0].result) res = out[0].result;
        } catch (e) {
          res = { list: [], itemsSeen: 0, sample: 'exec-error: ' + ((e && e.message) || e) };
        }
        const chatsJson = JSON.stringify(res.list || []);
        await sendToHost({
          version: '1', request_id: msg.request_id, source: 'extension',
          type: 'response', command: 'list_chats',
          result: {
            chats: chatsJson,
            itemsSeen: String(res.itemsSeen || 0),
            debug: res.sample || '',
            url: 'https://poe.com/chats'
          }
        });
      } catch (e) {
        await sendToHost({
          version: '1', request_id: msg.request_id, source: 'extension',
          type: 'error', command: 'list_chats', error: { message: (e && e.message) || 'LIST_CHATS_ERROR' }
        });
      }
    })();
    return;
  }

  const scrollCommands = new Set(['scrollGetMetrics', 'scrollSet', 'scrollBy', 'domQuery', 'domClick', 'windowSet', 'stopScroll', 'startKeepAlive', 'stopKeepAlive', 'validatePage', 'showError']);
  if (msg.type === 'request' && scrollCommands.has(msg.command)) {
      (async () => {
        try {
          // PHASE 1: Session Context Adoption
          // If the App provides a session context, we adopt it as the authoritative state.
          // This allows the App to drive the session (Stateless Remote Control) while
          // maintaining backward compatibility with the Extension's internal state management.
          if (!userCancelled && msg.args && msg.args.session) {
              const s = msg.args.session;
              // If no local session exists, or it's for a different tab, we adopt the App's context.
              if (!currentSession || currentSession.tabId !== s.tabId) {
                  currentSession = {
                      sessionId: `app-${s.tabId}`, // Stable synthetic ID
                      tabId: s.tabId,
                      windowId: s.windowId
                  };
              }
          }

          // STRICT SESSION CHECK
          const tabId = getTabIdFromMessage(msg);
          if (!tabId) {
             throw new Error('NO_ACTIVE_SESSION');
          }
          
          // WAIT FOR FOCUS LOGIC
          // stopScroll should not wait for focus as it is a cleanup command
      if (msg.command !== 'stopScroll') {
        if (currentSession && currentSession.tabId === tabId) {
            if (!(await waitForActiveSession(currentSession))) {
                throw new Error('TIMEOUT_WAITING_FOR_FOCUS');
            }
        }
      }
          
          const res = await execContent(tabId, {
            version: msg.version || '1',
            request_id: msg.request_id,
            type: 'request',
            command: msg.command,
            args: msg.args || {}
          });
          const result = flattenStringMap(res || {});
          await sendToHost({
              version: '1',
              request_id: msg.request_id,
              source: 'extension',
              type: 'response',
              command: msg.command,
              result: result
          });
        } catch (e) {
          const errMsg = e && e.message ? e.message : 'CONTENT_EXEC_ERROR';
          
          
          await sendToHost({
              version: '1',
              request_id: msg.request_id,
              source: 'extension',
              type: 'error',
              command: msg.command,
              args: null,
              error: { message: errMsg }
          });
        }
      })();
      return;
  }
    
    if (msg.type === 'response') {
        const tabId = getTabIdFromMessage(msg);
        if (tabId) {
            (async () => {
                const showOpenButton = msg.result && msg.result.showOpenButton === 'true';
                const message = msg.result && msg.result.message || UI_CONSTANTS.MESSAGES.EXPORT_COMPLETE;
                
                await execContent(tabId, { 
                    type: 'UPDATE_STATUS', 
                    options: { 
                        type: 'success', 
                        message: message, 
                        autoHideMs: 3000,
                        showOpenButton: showOpenButton
                    } 
                });
                clearSession(true);
            })();
        }
    }
    
    if (msg.type === 'error') {
        const tabId = getTabIdFromMessage(msg);
        if (tabId) {
             execContent(tabId, { 
                type: 'UPDATE_STATUS', 
                options: { type: 'error', message: UI_CONSTANTS.MESSAGES.EXPORT_FAILED_PREFIX + ": " + (msg.error?.message || UI_CONSTANTS.ERRORS.UNKNOWN) } 
            });
            // Reset state
            clearSession(true);
        }
    }
    


    if (msg.type === 'command' && msg.command === 'cancel_session') {
        (async () => {
          await clearSession(false);
        })();
    }
    if (msg.type === 'command' && msg.command === 'abort_export') {
        (async () => {
            const message = (msg.args && msg.args.message) || UI_CONSTANTS.ERRORS.UNKNOWN;
            const tabId = getTabIdFromMessage(msg);
            
            const session = currentSession;
            const sessionId = session ? session.sessionId : null;
            
            // Extract restoration metrics
            const restoreScrollTop = msg.args && msg.args.value; // Reused 'value' field
            const restoreWindowX = msg.args && msg.args.windowX;
            const restoreWindowY = msg.args && msg.args.windowY;
            
            try {
                if (tabId) {
                    // Restore if metrics are present
                    if (restoreScrollTop !== undefined || restoreWindowX !== undefined || restoreWindowY !== undefined) {
                         try {
                             if (restoreScrollTop !== undefined) {
                                await execContent(tabId, { command: 'scrollSet', args: { value: Number(restoreScrollTop) } });
                             }
                             if (restoreWindowX !== undefined || restoreWindowY !== undefined) {
                                await execContent(tabId, { command: 'windowSet', args: { windowX: Number(restoreWindowX||0), windowY: Number(restoreWindowY||0) } });
                             }
                         } catch (e) { console.error('Restoration failed:', e); }
                    }

                    await execContent(tabId, {
                        type: 'UPDATE_STATUS',
                        options: { type: 'error', message }
                    });
                    if (session && currentSession && currentSession.sessionId === sessionId && session.tabId === tabId) {
                        await clearSession(true);
                    }
                } else {
                    const tabs = await chrome.tabs.query({ active: true, currentWindow: true });
                    const t = tabs && tabs[0];
                    if (t) {
                        await execContent(t.id, {
                            type: 'UPDATE_STATUS',
                            options: { type: 'error', message }
                        });
                    }
                }
            } catch (e) {}
        })();
    }

    if (msg.type === 'command' && msg.command === 'focus_tab') {
        const tid = getTabIdFromMessage(msg);
        const wid = msg.args?.windowId;
        if (tid) {
            (async () => {
                try {
                    await chrome.tabs.update(tid, { active: true });
                    if (wid) {
                        await chrome.windows.update(wid, { focused: true });
                    } else {
                        const t = await chrome.tabs.get(tid);
                        await chrome.windows.update(t.windowId, { focused: true });
                    }
                } catch(e) { console.error('Focus tab failed:', e); }
            })();
        }
    }
    
    // 3.5 Handle Update UI Command (New)
    if (msg.type === 'command' && msg.command === 'update_ui') {
        const tabId = getTabIdFromMessage(msg);
        if (tabId) {
            const uiOptions = msg.args?.ui || {};
            (async () => {
                try {
                    await execContent(tabId, { type: 'UPDATE_STATUS', options: uiOptions });
                    
                    if (uiOptions.type === 'success') {
                        await clearSession(true);
                    }
                } catch (e) {}
            })();
        }
    }

    // 4. Handle UI Render Command (Phase 2)
    if (msg.type === 'command' && msg.command === 'ui_render') {
        const ui = msg.args?.ui;
        
        const tabId = getTabIdFromMessage(msg);
        
        if (tabId && ui) {
            (async () => {
                try {
                    await execContent(tabId, {
                        type: 'UI_RENDER',
                        ui: ui
                    });
                } catch (e) {}
            })();
        }
    }
}
async function waitForActiveSession(session) {
    let retries = 0;

    while (true) {
        // Fast fail if session was cleared externally (e.g. tab closed)
        if (!currentSession || currentSession.sessionId !== session.sessionId) {
             return false;
        }

        const isActive = await checkSessionActive(session);
        if (isActive) {
            if (retries > 0) {
                 await logToHost(`Resumed export on tab ${session.tabId}.`);
                 
                 // Send reset_timeout to host (App) to reset the grace period
                 await sendToHost({
                    version: '1', request_id: crypto.randomUUID(), source: 'extension', type: 'command',
                    command: 'reset_timeout', args: {}
                 });
                 
                 let resumeMsg = UI_CONSTANTS.MESSAGES.RESUMING_EXPORT;
                 // Assuming scrolling if not explicitly known, or app drives the message
                 
                 await execContent(session.tabId, {
                     type: 'UPDATE_STATUS',
                     options: { type: 'progress', message: resumeMsg, current: 0, total: 1 }
                 });
            }
            return true;
        }

        if (retries === 0) {
            await logToHost(`Paused export on tab ${session.tabId} due to focus loss.`);
        }

        retries++;
        // No timeout check here
        
        await new Promise(r => setTimeout(r, 100));
    }
}

// Helper to check if session is active/focused
async function checkSessionActive(session) {
    try {
        const tab = await chrome.tabs.get(session.tabId);
        if (!tab) return false;
        
        // Sync windowId if changed (e.g. detached/attached)
        if (tab.windowId !== session.windowId) {
            session.windowId = tab.windowId;
        }
        
        const win = await chrome.windows.get(session.windowId);
        // Relaxed check: Allow if window is visible (not minimized)
        // We don't strictly require 'focused' anymore to allow side-by-side multitasking.
        
        // The tab must still be the active tab in that window to ensure rendering priority
        // RELAXED: Allow background tabs for export
        
        return true;
    } catch (e) {
        return false;
    }
}

function flattenStringMap(obj, prefix = '') {
  const out = {};
  if (obj && typeof obj === 'object') {
    for (const [k, v] of Object.entries(obj)) {
      const key = prefix ? `${prefix}.${k}` : k;
      if (v && typeof v === 'object') {
        const nested = flattenStringMap(v, key);
        for (const [nk, nv] of Object.entries(nested)) out[nk] = String(nv);
      } else {
        out[key] = String(v);
      }
    }
  }
  return out;
}
async function getActiveWebTab() {
  let tabs = await chrome.tabs.query({ active: true, currentWindow: true })
  let t = tabs[0]
  if (t && /^https?:/i.test(t.url || '')) return t
  tabs = await chrome.tabs.query({ lastFocusedWindow: true })
  let web = tabs.find(x => /^https?:/i.test(x.url || ''))
  if (web) return web
  tabs = await chrome.tabs.query({})
  web = tabs.find(x => /^https?:/i.test(x.url || ''))
  if (web) return web
  throw new Error('NO_WEB_TAB')
}

// Navigate a tab to `url` and resolve once it finishes loading (plus a short
// settle for the SPA to render). Best-effort: resolves on timeout too.
async function navigateTabAndWait(tabId, url, timeoutMs = 40000) {
  await chrome.tabs.update(tabId, { url });
  await new Promise((resolve) => {
    let done = false;
    const finish = () => {
      if (done) return;
      done = true;
      try { chrome.tabs.onUpdated.removeListener(listener); } catch (e) {}
      clearTimeout(timer);
      resolve();
    };
    const listener = (id, info) => {
      if (id === tabId && info.status === 'complete') finish();
    };
    const timer = setTimeout(finish, timeoutMs);
    chrome.tabs.onUpdated.addListener(listener);
  });
  // Let the SPA hydrate / render the conversation or chat list.
  await new Promise((r) => setTimeout(r, 1500));
}

// Injected into the page's MAIN world (chrome.scripting world:'MAIN') to
// enumerate the chat history. Must be fully self-contained — no references to
// outer scope. Scrolls the infinite list to load every row (collecting as it
// goes, since rows recycle) and reads each chat's code from its React fiber
// (visible only in the MAIN world). Returns { list:[{url,title}], itemsSeen, sample }.
async function pageEnumerateChats() {
  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
  const ITEM = '[class*="ChatHistoryListItem_wrapper"]';
  const TITLE = '[class*="ChatHistoryListItem_title"]';
  const TRIGGER = '[class*="InfiniteScroll_pagingTrigger"]';

  // The MAIN history list (NOT the left sidebar / chat switcher, which also uses
  // ChatHistoryListItem and only holds ~recent chats).
  const mainContainer = () => {
    const secs = document.querySelectorAll('[class*="ChatHistoryPagedList_mainSection"]');
    if (secs.length) return secs[secs.length - 1];
    let best = null, bestN = -1;
    document.querySelectorAll('[class*="InfiniteScroll_container"]').forEach((c) => {
      const n = c.querySelectorAll(ITEM).length;
      if (n > bestN) { bestN = n; best = c; }
    });
    return best;
  };
  const items = () => {
    const c = mainContainer();
    return c ? c.querySelectorAll(ITEM) : document.querySelectorAll(ITEM);
  };

  const fiberOf = (el) => {
    const k = Object.keys(el).find(
      (k) => k.startsWith('__reactFiber$') || k.startsWith('__reactInternalInstance$')
    );
    return k ? el[k] : null;
  };
  // The URL slug is lowercase alphanumeric (e.g. 3g3zhkrgfvs92ggw736). The chat
  // object's `id` is an UPPERCASE base64 Relay GID (Chat:NNN) — must NOT be used.
  const SLUG = /^[a-z0-9]{12,30}$/;
  const isHashLen = (s) => s.length === 32 || s.length === 40 || s.length === 64;
  const findSlug = (props) => {
    const seen = new Set();
    const walk = (o, depth, inChat) => {
      if (!o || typeof o !== 'object' || depth > 5 || seen.has(o)) return null;
      if (o.nodeType || o.$$typeof) return null;
      seen.add(o);
      for (const k in o) {
        let v;
        try { v = o[k]; } catch (e) { continue; }
        if (typeof v === 'string') {
          // Primary: the row's link carries href="/chat/<slug>".
          const m = v.match(/\/chat\/([a-z0-9]+)/);
          if (m) return m[1];
          const key = k.replace(/_/g, '').toLowerCase();
          if ((key === 'code' || key === 'chatcode' || key === 'urlcode' || key === 'handle' || key === 'slug') && SLUG.test(v)) {
            return v;
          }
          if (inChat && SLUG.test(v) && !isHashLen(v) && key !== 'id' && !key.includes('hash') && !key.includes('title')) {
            return v;
          }
        }
      }
      for (const k in o) {
        let v;
        try { v = o[k]; } catch (e) { continue; }
        if (v && typeof v === 'object') {
          const r = walk(v, depth + 1, inChat || k.toLowerCase() === 'chat');
          if (r) return r;
        }
      }
      return null;
    };
    return walk(props, 0, false);
  };
  const slugOf = (item) => {
    let f = fiberOf(item), d = 0;
    while (f && d < 40) {
      const s = findSlug(f.memoizedProps);
      if (s) return s;
      f = f.return;
      d++;
    }
    return null;
  };

  const seen = new Map();
  const attempted = new WeakSet(); // avoid O(n^2) re-scans of the same row
  let itemsSeen = 0;
  const collect = () => {
    const list = items();
    itemsSeen = Math.max(itemsSeen, list.length);
    list.forEach((item) => {
      if (attempted.has(item)) return;
      attempted.add(item);
      const t = item.querySelector(TITLE);
      const title = t ? t.textContent.replace(/\s+/g, ' ').trim().slice(0, 200) : '';
      const slug = slugOf(item);
      if (!slug) return;
      const url = 'https://poe.com/chat/' + slug;
      if (!seen.has(url) || (!seen.get(url) && title)) seen.set(url, title);
    });
  };
  const getScroller = () => {
    const ms = document.querySelector('[class*="MainColumn_scrollSection"]');
    if (ms && ms.scrollHeight > ms.clientHeight + 20) return ms;
    const c = mainContainer();
    if (c && c.scrollHeight > c.clientHeight + 20) return c;
    return document.scrollingElement || document.documentElement;
  };

  for (let w = 0; w < 30 && items().length === 0; w++) await sleep(500);
  collect();
  let lastUrls = seen.size, lastRows = itemsSeen, stable = 0;
  const deadline = Date.now() + 300000; // hard 5-min budget so we always return
  // The main history list is an append list (rows stay in the DOM), so jump to
  // the bottom each step to load the next page, and be patient: only stop once
  // neither rows nor resolved URLs have grown for ~13s.
  for (let i = 0; i < 8000 && stable < 15 && Date.now() < deadline; i++) {
    const sc = getScroller();
    const trig = document.querySelector(TRIGGER);
    if (trig && trig.scrollIntoView) trig.scrollIntoView({ block: 'end' });
    sc.scrollTop = sc.scrollHeight;
    await sleep(900);
    collect();
    const grewUrls = seen.size !== lastUrls;
    const grewRows = itemsSeen !== lastRows;
    if (grewUrls) lastUrls = seen.size;
    if (grewRows) lastRows = itemsSeen;
    if (!grewUrls && !grewRows) stable++;
    else stable = 0;
  }

  // Diagnostic: if rows rendered but no slug resolved, dump the first MAIN row's
  // string props so we can see the exact slug field name.
  let sample = '';
  if (seen.size === 0 && itemsSeen > 0) {
    const item = items()[0];
    const f0 = item ? fiberOf(item) : null;
    if (!f0) {
      sample = 'no-fiber-key-in-main-world';
    } else {
      const hits = [], vis = new Set();
      const scan = (o, path, depth) => {
        if (!o || typeof o !== 'object' || depth > 4 || vis.has(o)) return;
        if (o.nodeType || o.$$typeof) return;
        vis.add(o);
        for (const k in o) {
          let v;
          try { v = o[k]; } catch (e) { continue; }
          if (typeof v === 'string' && v.length >= 4 && v.length <= 40) hits.push(path + '.' + k + '=' + v);
          else if (v && typeof v === 'object') scan(v, path + '.' + k, depth + 1);
        }
      };
      let f = f0, d = 0;
      while (f && d < 20) { if (f.memoizedProps) scan(f.memoizedProps, 'L' + d, 0); f = f.return; d++; }
      sample = hits.slice(0, 60).join(' | ') || 'no-strings';
    }
  }

  return { list: Array.from(seen, ([url, title]) => ({ url, title })), itemsSeen, sample };
}

async function execContent(tabId, message) {
  try {
    await chrome.tabs.get(tabId)
  } catch (e) {
    return null
  }
  try {
    return await chrome.tabs.sendMessage(tabId, message)
  } catch (e) {
    try {
      await chrome.scripting.executeScript({ target: { tabId }, files: ['ui-constants.js', 'content.js'] })
      return await chrome.tabs.sendMessage(tabId, message)
    } catch (er) {
        const msg = (er && er.message) ? er.message : ''
        if (/No tab with id/i.test(msg)) return null
        return null
    }
  }
}

 

async function sendPageHTML(tabId, targetFilename = 'dom.html', skipWrite = false, completionCommand = null, existingRequestId = null) {
  // If no tabId provided (legacy/button click without context?), try active
  if (!tabId) {
      try { 
          const t = await getActiveWebTab();
          tabId = t.id;
      } catch (e) { return { ok: false, error: 'NO_WEB_TAB' } }
  }
  
  try {
    // 1. Prepare HTML in content script
    const prepRes = await execContent(tabId, { 
        version: '1', 
        request_id: crypto.randomUUID(), 
        type: 'request', 
        command: 'preparePageHTML', 
        args: {} 
    })
    
    if (!prepRes || !prepRes.size) return { ok: false, error: 'NO_HTML_SIZE' }
    
    const totalSize = prepRes.size
    const CHUNK_SIZE = CONFIG.CHUNK_SIZE
    
    const rid = existingRequestId || crypto.randomUUID()
    
    // Send in chunks
    let offset = 0
    let chunkIndex = 0
    const totalChunks = Math.ceil(totalSize / CHUNK_SIZE)
    
    while (offset < totalSize) {
      const chunkRes = await execContent(tabId, {
        version: '1', request_id: crypto.randomUUID(), type: 'request',
        command: 'getPageHTMLChunk', args: { offset, size: CHUNK_SIZE }
      })
      
      if (!chunkRes || !chunkRes.chunkBase64) {
          return { ok: false, error: `FAILED_CHUNK_${chunkIndex}` }
      }
      
      await sendToHost({
        version: '1', request_id: rid, source: 'extension', type: 'request',
        command: 'saveDomHtmlChunk',
        args: { chunkBase64: chunkRes.chunkBase64, chunkIndex: chunkIndex, totalChunks: totalChunks }
      })
      
      offset += CHUNK_SIZE
      chunkIndex++
    }
    
    // Finalize
    if (!skipWrite) {
      if (completionCommand === 'capture_complete') {
          // App-driven flow
          const session = currentSession;
          const windowIdSnapshot = session ? session.windowId : 0;
          await sendToHost({
            version: '1', request_id: rid, source: 'extension', type: 'command',
            command: 'capture_complete',
            args: { 
                session: { tabId: tabId, windowId: windowIdSnapshot },
                totalChunks: totalChunks,
                chatTitle: targetFilename // Using passed name/title
            }
          })
      }
    }
    
    // Clear blob
    await execContent(tabId, { command: 'clearPageHTML' })
    
    return { ok: true, rid: rid }
  } catch (e) {
    return { ok: false, error: e.message }
  }
}

async function handleHostDownloadRequest(url) {
    try {
        await logToHost('Fetching asset...', url);
        // Step D: Fetch with browser caching (force-cache)
        // Omit credentials to avoid unnecessary auth overhead; rely on cache
        const response = await fetch(url, { 
            cache: "force-cache",
            credentials: 'omit',
            referrer: CONFIG.POE_BASE_URL
        });
        if (!response.ok) throw new Error(`Fetch failed: ${response.status}`);
        
        const blob = await response.blob();
        
        const CHUNK_SIZE = CONFIG.CHUNK_SIZE;
        const totalSize = blob.size;
        let offset = 0;
        let chunkIndex = 0;
        const totalChunks = Math.ceil(totalSize / CHUNK_SIZE) || 1;
        const rid = crypto.randomUUID(); // Consistent ID for this file
        
        if (totalSize === 0) {
             // Empty file
             await sendToHost({
                version: '1',
                request_id: rid,
                source: 'extension',
                type: 'request',
                command: 'save_file',
                args: {
                    url: url,
                    chunkBase64: "",
                    chunkIndex: 0,
                    totalChunks: 1
                }
            });
            // Send complete signal
            await sendToHost({
                version: '1',
                request_id: rid,
                source: 'extension',
                type: 'request',
                command: 'save_file_complete',
                args: { url: url }
            });
            return;
        }
        
        while (offset < totalSize) {
            const chunk = blob.slice(offset, offset + CHUNK_SIZE);
            const reader = new FileReader();
            
            const base64Chunk = await new Promise((resolve, reject) => {
                reader.onloadend = () => {
                    if (reader.error) reject(reader.error);
                    else resolve(reader.result.split(',')[1]);
                };
                reader.readAsDataURL(chunk);
            });
            
            await sendToHost({
                version: '1',
                request_id: rid,
                source: 'extension',
                type: 'request',
                command: 'save_file',
                args: {
                    url: url,
                    chunkBase64: base64Chunk,
                    chunkIndex: chunkIndex,
                    totalChunks: totalChunks
                }
            });
            
            offset += CHUNK_SIZE;
            chunkIndex++;
        }
        
        await sendToHost({
            version: '1',
            request_id: rid,
            source: 'extension',
            type: 'request',
            command: 'save_file_complete',
            args: { url: url }
        });
        
    } catch (e) {
        await logToHost(`Fetch error for ${url}: ${e.message}`);
        await sendToHost({
            version: '1',
            request_id: crypto.randomUUID(),
            source: 'extension',
            type: 'request',
            command: 'save_file_error',
            args: { url: url, message: e.message }
        });
    }
}



async function sendToHost(msg) {
    // Ensure connected
    if (!wsClient || wsClient.readyState !== WebSocket.OPEN) {
        await connectWebSocket();
    }
    
    if (wsClient && wsClient.readyState === WebSocket.OPEN) {
        try {
            wsClient.send(JSON.stringify(msg));
            return true;
        } catch (e) {
            console.error('WS Send Error:', e);
        }
    }
    
    return false;
}

async function logToHost(message, url) {
  await sendToHost({
    version: '1', request_id: crypto.randomUUID(), source: 'extension', type: 'request',
    command: 'log', args: { message, url }
  })
  return { ok: true }
}

async function logCurrentActiveTabURL(force = false) {
  try {
    if (inflightUrl !== null) {
      queuedForce = queuedForce || force;
      hasQueued = true;
      return;
    }
    let tabs = await chrome.tabs.query({ active: true, currentWindow: true });
    let t = tabs && tabs[0];
    let url = '';
    if (t && t.url) {
      url = t.url;
    } else {
      tabs = await chrome.tabs.query({ active: true });
      t = tabs && tabs[0];
      if (t && t.url) {
        url = t.url;
      }
    }
    if (!shouldSend(url, force)) return;
    inflightUrl = url;
    if (url) {
      const res = await logToHost('Active tab URL', url);
      if (res && res.ok) lastSentUrl = url;
      lastSentAt = Date.now();
    } else {
      const res = await logToHost('No active tab', '');
      if (res && res.ok) lastSentUrl = '';
      lastSentAt = Date.now();
    }
  } catch (e) {
    const url = '';
    if (!shouldSend(url, force)) return;
    const res = await logToHost('No active tab', '');
    if (res && res.ok) lastSentUrl = url;
    lastSentAt = Date.now();
  } finally {
    inflightUrl = null;
    if (hasQueued) {
      hasQueued = false;
      const f = queuedForce;
      queuedForce = false;
      setTimeout(() => { logCurrentActiveTabURL(f); }, 0);
    }
  }
}

function startLaunchPolling() {
    if (launchPollingInterval) clearInterval(launchPollingInterval);
    
    let attempts = 0;
    const maxAttempts = 60; // 30 seconds (if 500ms interval)
    
    launchPollingInterval = setInterval(async () => {
        attempts++;
        if (attempts > maxAttempts) {
            clearInterval(launchPollingInterval);
            launchPollingInterval = null;
            pendingAutoStart = null; // Give up
            return;
        }
        
        // Try connecting
        try {
            const ws = await connectWebSocket(CONFIG.TIMEOUT_RECONNECT_SHORT_MS);
            if (ws && ws.readyState === WebSocket.OPEN) {
                clearInterval(launchPollingInterval);
                launchPollingInterval = null;
                
                // Connected!
                if (pendingAutoStart) {
                    const { tabId, windowId, url } = pendingAutoStart;
                    pendingAutoStart = null;
                    
                    // 1. Restore Tab Focus
                    try {
                        await chrome.windows.update(windowId, { focused: true });
                        await chrome.tabs.update(tabId, { active: true });
                    } catch (e) {
                        console.error('Failed to restore tab focus:', e);
                    }
                    
                    // 2. Start Export
                    setTimeout(() => {
                        startExport({ id: tabId, windowId, url });
                    }, 500);
                }
            }
        } catch (e) {
            // Ignore connection errors during polling
        }
    }, 500);
}

// --- Main Listener ---

function isPoeChat(url) {
    if (!url) return false;
    try {
        const u = new URL(url);
        if (!u.hostname.endsWith('poe.com') && !u.hostname.endsWith('poecdn.net')) return false;
        // Strictly allow only /chat/ pages as they contain the actual conversations
        return u.pathname.startsWith('/chat/');
    } catch (e) { return false; }
}

async function updateActionState(tabId, url) {
    // Always show the popup so the user can choose "Export this chat" vs
    // "Export ALL chats" everywhere on poe.com. (Previously chat pages and the
    // active export tab suppressed the popup for one-click export, which hid the
    // "Export ALL" option — and a stale session could leave it suppressed.)
    try {
        await chrome.action.setPopup({ tabId: tabId, popup: 'popup.html' });
    } catch (e) {}
}

chrome.tabs.onUpdated.addListener((tabId, changeInfo, tab) => {
    if (changeInfo.status === 'complete' && tab.url) {
        updateActionState(tabId, tab.url);
    }
});

chrome.tabs.onActivated.addListener(async (activeInfo) => {
    try {
        const tab = await chrome.tabs.get(activeInfo.tabId);
        if (tab.url) {
            updateActionState(activeInfo.tabId, tab.url);
        }
    } catch (e) {}
});

chrome.action.onClicked.addListener(async (tab) => {
      // Triggered ONLY when popup is set to '' (i.e., on valid Poe Chat pages)
      
      
      // 1. Check if already exporting in this tab
      if (currentSession && currentSession.tabId === tab.id) {
      }
      
      // 2. Check App Connection & Start
      // We try to connect. If fails, we launch the app.
      const ws = await connectWebSocket(CONFIG.TIMEOUT_RECONNECT_SHORT_MS);
      
      if (ws && ws.readyState === WebSocket.OPEN) {
          // App is running -> Start Export
          await startExport(tab);
      } else {
          // App NOT running -> Route to popup (no auto-launch)
          try {
              await chrome.action.setPopup({ tabId: tab.id, popup: 'popup.html' });
              if (chrome.action.openPopup) {
                  await chrome.action.openPopup({ windowId: tab.windowId });
              }
          } catch (e) {}
          return;
      }
  });

function checkDestinationReady() {
    return new Promise((resolve) => {
        const rid = crypto.randomUUID();
        const timeout = setTimeout(() => {
             responseListeners.delete(rid);
             // Timeout: assume everything is fine or let App handle errors later
             resolve({});
        }, 2500);
        
        responseListeners.set(rid, (msg) => {
            clearTimeout(timeout);
            responseListeners.delete(rid);
            resolve(msg.result || {});
        });
        
        sendToHost({
            version: '1', request_id: rid, source: 'extension', type: 'request',
            command: 'check_destination', args: {}
        });
    });
}

async function getExportTabVisibility() {
    const session = currentSession;
    if (!session) return { isBackground: true };
    try {
        const tab = await chrome.tabs.get(session.tabId);
        const win = await chrome.windows.get(session.windowId);
        const isForeground = !!(tab && tab.active && win && win.focused);
        return { isBackground: !isForeground };
    } catch (e) {
        return { isBackground: true };
    }
}

async function getVisibilityFor(tabId, windowId) {
    try {
        const tab = await chrome.tabs.get(tabId);
        const win = await chrome.windows.get(windowId);
        const isForeground = !!(tab && tab.active && win && win.focused);
        return { isBackground: !isForeground };
    } catch (e) {
        return { isBackground: true };
    }
}

async function startExport(tab, skipValidation = false, isBulk = false) {
      userCancelled = false;
      // 1. Validate Page
      const isPoeChat = (() => {
          if (!tab.url) return false;
          try {
              const u = new URL(tab.url);
              if (!u.hostname.endsWith('poe.com') && !u.hostname.endsWith('poecdn.net')) return false;
              
              // Strictly allow only /chat/ pages as they contain the actual conversations
              // Bot landing pages (e.g. poe.com/Claude-3-Opus) are initial states without user messages
              if (u.pathname.startsWith('/chat/')) return true; 
              
              return false;
          } catch (e) { return false; }
      })();
  
      if (!isPoeChat) {
        await execContent(tab.id, { 
          type: 'UPDATE_STATUS', 
          options: { type: 'warning', message: UI_CONSTANTS.ERRORS.ONLY_POE_PAGES, autoHideMs: 5000 } 
        });
        return;
      }
  
      // 1.5 Content Validation (for 1-click export)
      if (!skipValidation) {
          try {
              const valRes = await execContent(tab.id, { command: 'validatePage' });
              if (valRes && valRes.ok === 'false') {
                   await chrome.action.setPopup({ tabId: tab.id, popup: 'popup.html' });
                   if (chrome.action.openPopup) {
                       await chrome.action.openPopup({ windowId: tab.windowId });
                   }
                   return;
              }
          } catch (e) {
               console.warn('Content validation error:', e);
          }
      }
      
      // 2. Connect & Invoke (Connect FIRST to ask App about Destination)
      const ws = await connectWebSocket();
      if (!ws || ws.error) {
        const errorMsg = ws?.error || UI_CONSTANTS.ERRORS.APP_NOT_RUNNING;
      await execContent(tab.id, { 
        type: 'UPDATE_STATUS', 
        options: { type: 'error', message: errorMsg } 
      });
      try {
          const tRes = await execContent(tab.id, { command: 'getChatTitle' });
          const chatTitle = tRes && tRes.title ? String(tRes.title) : '';
          const vis = await getVisibilityFor(tab.id, tab.windowId);
          await sendToHost({
              version: '1', request_id: crypto.randomUUID(), source: 'extension', type: 'command',
              command: 'abort_export', args: { message: errorMsg, chatTitle, isBackground: vis.isBackground, tabId: tab.id, windowId: tab.windowId }
          });
      } catch (e) {
          const vis = await getVisibilityFor(tab.id, tab.windowId);
          await sendToHost({
              version: '1', request_id: crypto.randomUUID(), source: 'extension', type: 'command',
              command: 'abort_export', args: { message: errorMsg, isBackground: vis.isBackground, tabId: tab.id, windowId: tab.windowId }
          });
      }
      return;
    }

    // 2.5 Check Destination Status via App
    const destStatus = await checkDestinationReady();
    
    if (destStatus && destStatus.message) {
        const msg = destStatus.message;
        await logToHost(`Destination check (start): error="${msg}"`);
        await execContent(tab.id, { type: 'UPDATE_STATUS', options: { type: 'error', message: msg } });
        try {
            const tRes = await execContent(tab.id, { command: 'getChatTitle' });
            const chatTitle = tRes && tRes.title ? String(tRes.title) : '';
            const vis = await getVisibilityFor(tab.id, tab.windowId);
            await sendToHost({
                version: '1', request_id: crypto.randomUUID(), source: 'extension', type: 'command',
                command: 'abort_export', args: { message: msg, chatTitle, isBackground: vis.isBackground, tabId: tab.id, windowId: tab.windowId }
            });
        } catch (e) {
            const vis = await getVisibilityFor(tab.id, tab.windowId);
            await sendToHost({
                version: '1', request_id: crypto.randomUUID(), source: 'extension', type: 'command',
                command: 'abort_export', args: { message: msg, isBackground: vis.isBackground, tabId: tab.id, windowId: tab.windowId }
            });
        }
        return;
    }
    
    await logToHost('Active tab URL', tab.url);

    // 2.5 Check Lock
    const session = currentSession;
    if (session) {
        let sessionTabExists = false;
        try {
            await chrome.tabs.get(session.tabId);
            sessionTabExists = true;
        } catch (e) {
            if (currentSession && currentSession.sessionId === session.sessionId) {
                currentSession = null;
            }
        }

        if (sessionTabExists) {
            if (session.tabId === tab.id) {
                await sendToHost({
                    version: '1', request_id: crypto.randomUUID(), source: 'extension', type: 'command',
                    command: 'abort_export', args: { message: 'Restarted by user', tabId: session.tabId, windowId: session.windowId }
                });
                
                if (currentSession && currentSession.sessionId === session.sessionId) {
                    await clearSession(false);
                }
            } else {
                try {
                    await chrome.action.setPopup({ tabId: tab.id, popup: 'popup.html' });
                } catch (e) {}
                return;
            }
        }
    }
    
    // 2.7 Check Version Compatibility
    if (versionCheckError) {
        await logToHost(`Export blocked due to version mismatch: ${versionCheckError}`);
        await execContent(tab.id, { 
            type: 'UPDATE_STATUS', 
            options: { type: 'error', message: versionCheckError } 
        });
        return;
    }

    // 3. Send Invoke Command
    await sendToHost({
        version: '1',
        request_id: crypto.randomUUID(),
        source: 'extension',
        type: 'command',
        command: isBulk ? 'invoke_bulk_export' : 'invoke_export',
        args: { tabId: tab.id, windowId: tab.windowId }
    });

    // Initialize state
    currentSession = {
        sessionId: crypto.randomUUID(),
        tabId: tab.id,
        windowId: tab.windowId
    };
    // session tracked in-memory only
    
    // Disable popup for this tab (use toast instead)
    try {
        await chrome.action.setPopup({ tabId: tab.id, popup: '' });
    } catch (e) { console.error('Failed to set popup', e); }
    
    await execContent(tab.id, { 
      type: 'UPDATE_STATUS', 
      options: { 
        type: 'progress', 
        message: UI_CONSTANTS.POPUP.STATUS.LOADING_HISTORY, 
        current: 0, 
        total: 1 
      } 
    });
}

chrome.runtime.onInstalled.addListener(() => {
    connectWebSocket();
});

chrome.runtime.onConnect.addListener((port) => {
    if (port.name === 'keepAlive') {
        port.onMessage.addListener((msg) => {
             if (msg.type === 'PING') {
                 // Ensure WS connection
                 if (!wsClient || wsClient.readyState !== WebSocket.OPEN) {
                     connectWebSocket();
                 }
                 return;
             }
             if (msg.type === 'KEEP_ALIVE') {
                 // Propagate to Host
                 sendToHost({
                    version: '1',
                    request_id: 'keep-alive-' + Date.now(),
                    source: 'extension',
                    type: 'ping',
                    command: 'ping',
                    args: {}
                 }).catch((e) => {
                     console.warn('[Background] Failed to send ping to host:', e);
                 });
             }
        });
    }
});

chrome.runtime.onMessage.addListener((msg, sender, sendResponse) => {
  (async () => {

    // --- Popup Handlers ---

    // Handle logs from content script
    if (msg && msg.type === 'log') {
        const message = msg.message;
        const details = msg.details;
        await logToHost(message, details);
        sendResponse({ ok: true });
        return;
    }

    if (msg && msg.type === 'GET_SESSION_STATUS') {
      (async () => {
          let wsActive = wsClient && wsClient.readyState === WebSocket.OPEN;
          
          // If session exists but WS is dead, try to reconnect ONCE before giving up
          if (!wsActive) {
              // OPTIMIZATION: Non-blocking reconnection
              // Fire-and-forget attempt to reconnect for NEXT poll
              const now = Date.now();
              const CONNECTION_COOLDOWN_MS = 2000;
              
              if (now - lastConnectionAttempt > CONNECTION_COOLDOWN_MS) {
                  lastConnectionAttempt = now;
                  connectWebSocket(CONFIG.TIMEOUT_RECONNECT_SHORT_MS).catch(() => {});
              }
              
              // Fail fast for NOW
              sendResponse({ active: false, error: UI_CONSTANTS.ERRORS.APP_NOT_RUNNING });
              return;
          }
          
          // 1. Try App Query if connected
          if (wsActive) {
               const requestId = crypto.randomUUID();
               const statusPromise = new Promise((resolve) => {
                   const timeout = setTimeout(() => {
                       resolve(null);
                   }, 250);
                   pendingSessionStatusCallbacks[requestId] = (response) => {
                       clearTimeout(timeout);
                       resolve(response);
                   };
               });
               
               sendToHost({
                  version: '1',
                  command: 'get_session_status',
                  type: 'request',
                  request_id: requestId,
                  source: 'extension',
                  args: {}
               });
               
               const appStatus = await statusPromise;
               
               if (appStatus) {
                    if (appStatus.result?.active === 'true') {
                        sendResponse({ 
                            active: true, 
                            tabId: parseInt(appStatus.result.tabId),
                            windowId: parseInt(appStatus.result.windowId),
                            status: appStatus.result.status,
                            current: parseInt(appStatus.result.current || '0'),
                            total: parseInt(appStatus.result.total || '0')
                        });
                    } else {
                        if (currentSession) {
                            await clearSession(true, true);
                        }
                        sendResponse({ active: false });
                    }
                    return;
               }
          }
          
          console.warn('[GET_SESSION_STATUS] App unreachable, reporting inactive');
          sendResponse({ active: false, error: UI_CONSTANTS.ERRORS.APP_NOT_RUNNING });
      })();
      return true; // Async response
    }

    if (msg && msg.type === 'EXPECT_APP_LAUNCH') {
        pendingAutoStart = {
            tabId: msg.tabId,
            windowId: msg.windowId,
            url: msg.url,
            timestamp: Date.now()
        };
        startLaunchPolling();
        sendResponse({ ok: true });
        return;
    }

    if (msg && msg.type === 'CANCEL_EXPORT') {
      (async () => {
        userCancelled = true;
        
        // Notify Host to Request Abort
        // The host will respond with 'abort_export' which triggers restoration.
        try {
            await logToHost('User cancelled export. Requesting abort.');
            await sendToHost({
                version: '1', request_id: crypto.randomUUID(), source: 'extension', type: 'command',
                command: 'request_abort', args: { message: 'User cancelled', tabId: currentSession?.tabId, windowId: currentSession?.windowId }
            });
        } catch (e) {}
      })();
      sendResponse({ ok: true });
      return;
    }

    if (msg && msg.type === 'OPEN_RESULT') {
        sendToHost({
            version: '1', request_id: crypto.randomUUID(), source: 'extension', type: 'command',
            command: 'open_result', args: {}
        }).catch(() => {});
        sendResponse({ ok: true });
        return;
    }

    if (msg && msg.type === 'VISIBILITY_CHANGED') {
      const hidden = msg.hidden;
      if (currentSession && sender.tab && currentSession.tabId === sender.tab.id) {
          logToHost(`Tab visibility changed. Hidden: ${hidden}`);
          sendToHost({
              version: '1', request_id: crypto.randomUUID(), source: 'extension', type: 'command',
              command: 'update_tab_status', args: { isBackground: hidden }
          }).catch(() => {});
          
          // Also trigger reset_timeout if becoming visible (redundant but safe)
          if (!hidden) {
               sendToHost({
                   version: '1', request_id: crypto.randomUUID(), source: 'extension', type: 'command',
                   command: 'reset_timeout', args: {}
               }).catch(() => {});
          }
      }
      sendResponse({ ok: true });
      return;
    }


    if (msg && msg.type === 'FOCUS_EXPORT_TAB') {
      (async () => {
        if (currentSession) {
          try {
            // Focus window first
            await chrome.windows.update(currentSession.windowId, { focused: true });
            // Focus tab
            await chrome.tabs.update(currentSession.tabId, { active: true });
          } catch (e) {
            console.error('Focus failed:', e);
          }
        }
      })();
      sendResponse({ ok: true });
      return;
    }

    if (msg && msg.type === 'START_EXPORT') {
        (async () => {
            const tab = {
                id: msg.tabId,
                windowId: msg.windowId,
                url: msg.url
            };
            await startExport(tab, true);
        })();
        sendResponse({ ok: true });
        return;
    }

    if (msg && msg.type === 'START_BULK_EXPORT') {
        (async () => {
            const tab = {
                id: msg.tabId,
                windowId: msg.windowId,
                url: msg.url
            };
            // Bulk export: the app enumerates all chats (list_chats) and drives
            // navigate + capture for each.
            await startExport(tab, true, true);
        })();
        sendResponse({ ok: true });
        return;
    }
    
    if (msg && msg.type === 'VALIDATE_PAGE') {
        (async () => {
            try {
                const tabs = await chrome.tabs.query({ active: true, currentWindow: true });
                const tab = tabs && tabs[0];
                
                if (!tab || !tab.id) {
                    sendResponse({ ok: false, error: 'No active tab' });
                    return;
                }
                
                const result = await execContent(tab.id, { command: 'validatePage' });
                sendResponse(result || { ok: false });
            } catch (e) {
                sendResponse({ ok: false, error: e.message });
            }
        })();
        return true;
    }

    if (msg && msg.type === 'RETRY_EXPORT') {
        (async () => {
            try {
                const tabs = await chrome.tabs.query({ active: true, currentWindow: true });
                const t = tabs && tabs[0];
                if (t) {
                    await startExport({ id: t.id, windowId: t.windowId, url: t.url });
                }
            } catch (e) {}
        })();
        sendResponse({ ok: true });
        return;
    }


    sendResponse({ ok: false })
  })()
  return true
})

// Initialize state

chrome.tabs.onRemoved.addListener((tabId) => {
    if (currentSession && currentSession.tabId === tabId) {
        // Notify host to abort
        sendToHost({
            version: '1', request_id: crypto.randomUUID(), source: 'extension', type: 'command',
            command: 'abort_export', args: { message: 'Tab closed', tabId: currentSession.tabId, windowId: currentSession.windowId }
        });
        
        logToHost(`Tab ${tabId} closed. Export aborted.`);
        clearSession();
    }
});

chrome.tabs.onUpdated.addListener((tabId, changeInfo, tab) => {
    if (currentSession && currentSession.tabId === tabId) {
        // If the page is reloading/navigating (status goes to loading), abort the export
        if (changeInfo.status === 'loading') {
            // Notify host to abort
            sendToHost({
                version: '1', request_id: crypto.randomUUID(), source: 'extension', type: 'command',
                command: 'abort_export', args: { message: 'Navigated away', tabId: currentSession.tabId, windowId: currentSession.windowId }
            });
            logToHost(`Tab ${tabId} navigated away. Export aborted.`);
            clearSession();
        }
    }
});

// --- Focus / Activity Monitoring (Reset Timeout) ---

chrome.tabs.onActivated.addListener(async (activeInfo) => {
    if (currentSession && currentSession.tabId === activeInfo.tabId) {
        // Export tab became active
        await sendToHost({
            version: '1', request_id: crypto.randomUUID(), source: 'extension', type: 'command',
            command: 'reset_timeout', args: {}
        });
    }
});

chrome.windows.onFocusChanged.addListener(async (windowId) => {
    if (windowId === chrome.windows.WINDOW_ID_NONE) return;
    
    if (currentSession && currentSession.windowId === windowId) {
        // Window focused. Check if active tab is ours.
        try {
            const tab = await chrome.tabs.get(currentSession.tabId);
            if (tab && tab.active) {
                await sendToHost({
                    version: '1', request_id: crypto.randomUUID(), source: 'extension', type: 'command',
                    command: 'reset_timeout', args: {}
                });
            }
        } catch (e) {}
    }
});
