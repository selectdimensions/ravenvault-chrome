/*
 * MIT License
 * Copyright (c) 2026 RavenVault
 * See LICENSE file for details.
 */

// Centralized UI configuration and text strings
//
// Fallback strings used when the app is unreachable

if (typeof UI_CONSTANTS === 'undefined') {
    var UI_CONSTANTS = {
        // Strings used in the Popup (popup.html / popup.js)
        POPUP: {
            TITLE: {
                ALREADY_IN_PROGRESS: "Export in progress",
                NO_ACTIVE_TAB: "No conversation found",
                ERROR: "Connection error"
            },
            STATUS: {
                LOADING_HISTORY: "Loading history...",
                BUSY_ANOTHER_TAB: "Another tab is exporting.",
                NO_ACTIVE_TAB_HINT: "Open a chat on poe.com to export.",
                APP_NOT_OPEN: "Open Poe2Obsidian to continue."
            },
            BUTTONS: {
                RETURN_TO_EXPORT: "Show",
                DISMISS: "Dismiss",
                LAUNCH_APP: "Launch App",
                GO_TO_POE: "Go to Poe"
            }
        },

        // Strings used in the Content Script (content.js) and Overlay
        OVERLAY: {
            TITLE: "Loading conversation",
            SUBTEXT: "Scrolling to capture all messages",
            BUTTON_CANCEL: "Cancel",
            KEEP_TAB_OPEN: "Keep this tab open until complete"
        },

        // Strings used in Toasts / Status Updates (background.js -> content.js)
        MESSAGES: {
            EXPORT_COMPLETE: "Export complete",
            EXPORT_FAILED_PREFIX: "Export failed",
            EXPORT_ABORTED_TIMEOUT: "Export cancelled",
            TIMED_OUT_WAITING: "Export cancelled",
            CONNECTION_LOST_INACTIVE: "Export cancelled",
            PREPARING_EXPORT: "Preparing export...",

            RESUMING_EXPORT: "Resuming export...",
        },

        // Error Messages (background.js)
        ERRORS: {
            CONNECTION_FAILED: "Unable to connect to Poe2Obsidian. Please make sure the desktop app is open and try again.",
            CONNECTION_CLOSED: "Connection to Poe2Obsidian was closed.",
            CONNECTION_TIMED_OUT: "Connection timed out. Please check if Poe2Obsidian is open.",
            APP_NOT_RUNNING: "Poe2Obsidian is not running. Please open the desktop app.",
            ONLY_POE_PAGES: "Poe2Obsidian only works on Poe.com chat pages.",
            EXPORT_IN_PROGRESS_OTHER_TAB: "Poe2Obsidian is already exporting in another tab.",
            CAPTURE_HTML_FAILED: "Something went wrong capturing the chat: ",
            UNKNOWN: "Unknown error",
            
            // Version Errors
            APP_OUTDATED: "Poe2Obsidian Desktop App is outdated. Please update to continue.",
            EXTENSION_OUTDATED: "Poe2Obsidian Extension is outdated. Please update via Chrome Web Store."
        }
    };
}
