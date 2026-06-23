/*
 * MIT License
 * Copyright (c) 2026 RavenVault
 * See LICENSE file for details.
 */

document.addEventListener('DOMContentLoaded', async () => {
    // UI Elements
    const elTitle = document.getElementById('popupTitle');
    const elStatus = document.getElementById('popupStatus');
    const btnPrimary = document.getElementById('btnPrimary');
    const btnSecondary = document.getElementById('btnSecondary');
    const iconContainer = document.getElementById('iconContainer');
    const iconSvg = document.getElementById('iconSvg');
    const elDownloadContainer = document.getElementById('downloadContainer');
    const elDownloadLink = document.getElementById('downloadLink');

    function updateIcon(type) {
        // type: 'busy', 'error', 'neutral'
        iconContainer.className = 'icon-container'; // reset
        iconSvg.innerHTML = ''; // clear paths
        iconSvg.classList.remove('spin');

        if (type === 'busy') {
            iconContainer.classList.add('icon-busy');
            iconSvg.innerHTML = '<path d="M21 12a9 9 0 1 1-6.219-8.56" />';
            iconSvg.classList.add('spin');
        } else if (type === 'error') {
            iconContainer.classList.add('icon-error');
            iconSvg.innerHTML = '<circle cx="12" cy="12" r="10"></circle><line x1="12" y1="8" x2="12" y2="12"></line><line x1="12" y1="16" x2="12.01" y2="16"></line>';
        } else {
            // neutral / default (File)
            iconContainer.classList.add('icon-neutral');
            iconSvg.innerHTML = '<path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"></path><polyline points="14 2 14 8 20 8"></polyline><line x1="16" y1="13" x2="8" y2="13"></line><line x1="16" y1="17" x2="8" y2="17"></line><polyline points="10 9 9 9 8 9"></polyline>';
        }
    }

    function updateUI(state) {
        elTitle.textContent = state.title;
        elStatus.textContent = state.status || '';
        
        updateIcon(state.icon || 'neutral');

        if (state.primary) {
            btnPrimary.textContent = state.primary;
            btnPrimary.onclick = state.primaryAction;
            btnPrimary.classList.remove('hidden');
        } else {
            btnPrimary.classList.add('hidden');
        }

        if (state.secondary) {
            btnSecondary.textContent = state.secondary;
            btnSecondary.onclick = state.secondaryAction;
            btnSecondary.classList.remove('hidden');
        } else {
            btnSecondary.classList.add('hidden');
        }

        if (state.downloadUrl) {
            elDownloadLink.href = state.downloadUrl;
            elDownloadContainer.classList.remove('hidden');
        } else {
            elDownloadContainer.classList.add('hidden');
        }
    }

    // Helper to get active tab
    const [currentTab] = await chrome.tabs.query({ active: true, currentWindow: true });
    
    // Strict validation: Only match poe.com/chat/ URLs
    const isPoe = (() => {
        if (!currentTab || !currentTab.url) return false;
        try {
            const u = new URL(currentTab.url);
            if (!u.hostname.endsWith('poe.com') && !u.hostname.endsWith('poecdn.net')) return false;
            return u.pathname.startsWith('/chat/');
        } catch (e) { return false; }
    })();

    let wasActive = false;
    let isRefreshing = false;

    async function refreshState() {
        if (isRefreshing) return;
        isRefreshing = true;
        let res;
        try {
            try {
                // Add race against timeout in case background script is hung
            const timeoutPromise = new Promise((_, reject) => 
                setTimeout(() => reject(new Error("Timeout")), 300)
            );
            const sendPromise = chrome.runtime.sendMessage({ type: 'GET_SESSION_STATUS' });
            res = await Promise.race([sendPromise, timeoutPromise]);
            } catch (e) {
                console.error("Popup communication error:", e);
                // Fallback: Assume connection error if communication fails
                res = { active: false, error: UI_CONSTANTS.ERRORS.APP_NOT_RUNNING };
            }
            
            if (res && res.active) {
            wasActive = true;
            // Session Exists.
            // Note: If we are on the active export tab, the popup is disabled by background.js,
            // so this code only runs when we are on a DIFFERENT tab.
            
            // Always show "Busy/Already Running" state when an export is active elsewhere
            updateUI({
                title: UI_CONSTANTS.POPUP.TITLE.ALREADY_IN_PROGRESS,
                status: UI_CONSTANTS.POPUP.STATUS.BUSY_ANOTHER_TAB,
                icon: 'busy',
                primary: UI_CONSTANTS.POPUP.BUTTONS.RETURN_TO_EXPORT,
                primaryAction: () => {
                    chrome.runtime.sendMessage({ type: 'FOCUS_EXPORT_TAB' });
                    window.close();
                },
                secondary: UI_CONSTANTS.POPUP.BUTTONS.DISMISS,
                secondaryAction: () => window.close()
            });
        } else {
            // No Active Session
            if (wasActive) {
                // If we were active and now are not, export finished.
                window.close();
                return;
            }

            // Check for App Connection Error FIRST
            if (res?.error) {
                

                updateUI({
                    title: UI_CONSTANTS.POPUP.TITLE.ERROR, // or "Connection Error" / "App not running"
                    status: res.error === UI_CONSTANTS.ERRORS.APP_NOT_RUNNING ? UI_CONSTANTS.POPUP.STATUS.APP_NOT_OPEN : res.error,
                    icon: 'error',
                    primary: res.error === UI_CONSTANTS.ERRORS.APP_NOT_RUNNING ? UI_CONSTANTS.POPUP.BUTTONS.LAUNCH_APP : UI_CONSTANTS.POPUP.BUTTONS.DISMISS,
                    downloadUrl: res.error === UI_CONSTANTS.ERRORS.APP_NOT_RUNNING ? 'https://ravenvault.app' : undefined,
                    primaryAction: () => {
                         if (res.error === UI_CONSTANTS.ERRORS.APP_NOT_RUNNING) {
                             // Notify background to expect app launch and auto-start
                             chrome.runtime.sendMessage({ 
                                 type: 'EXPECT_APP_LAUNCH', 
                                 tabId: currentTab.id, 
                                 windowId: currentTab.windowId, 
                                 url: currentTab.url 
                             });
                             
                             chrome.tabs.create({ url: 'ravenvault://open' });
                             window.close();
                         } else {
                             window.close();
                         }
                    }
                });
                return;
            }

            // Validate page content before starting
            let shouldStartExport = false;
            if (isPoe) {
                 try {
                    const valPromise = chrome.runtime.sendMessage({ type: 'VALIDATE_PAGE' });
                    const timeoutVal = new Promise((_, reject) => setTimeout(() => reject(new Error('Timeout')), 800));
                    const valRes = await Promise.race([valPromise, timeoutVal]);
                    
                    if (valRes && valRes.ok === 'true') {
                        shouldStartExport = true;
                    }
                } catch (e) {
                    // Validation check failed
                }
            }

            if (shouldStartExport) {
                // Offer a choice: export just this conversation, or every chat.
                updateUI({
                    title: 'Ready to export',
                    status: 'Export this conversation, or all of your Poe chats.',
                    icon: 'neutral',
                    primary: 'Export this chat',
                    primaryAction: () => {
                        chrome.runtime.sendMessage({ type: 'START_EXPORT', tabId: currentTab.id, windowId: currentTab.windowId, url: currentTab.url });
                        window.close();
                    },
                    secondary: 'Export ALL chats',
                    secondaryAction: () => {
                        chrome.runtime.sendMessage({ type: 'START_BULK_EXPORT', tabId: currentTab.id, windowId: currentTab.windowId, url: currentTab.url });
                        window.close();
                    }
                });
            } else if (isPoe) {
                // On a Poe page that isn't a single exportable conversation
                // (e.g. the History page poe.com/chats). Offer bulk export —
                // it navigates to /chats and walks every conversation itself.
                updateUI({
                    title: 'Export all chats',
                    status: 'Export your entire Poe history to your vault.',
                    icon: 'neutral',
                    primary: 'Export ALL chats',
                    primaryAction: () => {
                        chrome.runtime.sendMessage({ type: 'START_BULK_EXPORT', tabId: currentTab.id, windowId: currentTab.windowId, url: currentTab.url });
                        window.close();
                    },
                    secondary: UI_CONSTANTS.POPUP.BUTTONS.DISMISS,
                    secondaryAction: () => window.close()
                });
            } else {
                // Not a Poe page at all.
                updateUI({
                    title: UI_CONSTANTS.POPUP.TITLE.NO_ACTIVE_TAB,
                    status: UI_CONSTANTS.POPUP.STATUS.NO_ACTIVE_TAB_HINT,
                    icon: 'neutral',
                    primary: UI_CONSTANTS.POPUP.BUTTONS.GO_TO_POE,
                    primaryAction: () => {
                        chrome.tabs.create({ url: 'https://poe.com' });
                        window.close();
                    }
                });
            }
        }
        } finally {
            isRefreshing = false;
        }
    }

    // Initial check
    await refreshState();
    
    // Poll for updates
    setInterval(refreshState, 500);
});
