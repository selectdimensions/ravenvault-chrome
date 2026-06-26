/*
 * MIT License
 * Copyright (c) 2026 RavenVault
 * See LICENSE file for details.
 */

/**
 * Centralized configuration for the Poe2Obsidian Extension.
 * This file contains all hardcoded values used in the extension background and content scripts.
 */

const CONFIG = {
    // MARK: - Network Configuration
    
    // Using 127.0.0.1 avoids DNS resolution issues (localhost -> ::1) and is faster.
    WEBSOCKET_URL: 'ws://127.0.0.1:53122',
    
    /**
     * Base URL for Poe.com.
     * Usage: `referrer: CONFIG.POE_BASE_URL`
     */
    POE_BASE_URL: 'https://poe.com/',
    
    // MARK: - Timeouts
    
    /**
     * Timeout for WebSocket connection attempts (in milliseconds).
     * Usage: `connectWebSocket(CONFIG.TIMEOUT_CONNECT_MS)`
     */
    TIMEOUT_CONNECT_MS: 5000,
    
    /**
     * Short timeout for quick reconnection attempts (in milliseconds).
     * Usage: `connectWebSocket(CONFIG.TIMEOUT_RECONNECT_SHORT_MS)`
     */
    TIMEOUT_RECONNECT_SHORT_MS: 500,
    
    // MARK: - Limits
    
    /**
     * Chunk size for file transfers (in bytes). 256KB.
     * Usage: `const CHUNK_SIZE = CONFIG.CHUNK_SIZE;`
     */
    CHUNK_SIZE: 1024 * 256,
    
    // MARK: - Version Requirements
    
    /**
     * The minimum required version for the native App.
     * The Extension checks the App's version during the handshake.
     * If the App is older than this, usage is blocked to prevent protocol errors.
     * Usage: `CONFIG.MIN_APP_VERSION`
     */
    MIN_APP_VERSION: "0.9.1"
};

// Export for usage in ES modules or direct inclusion
if (typeof module !== 'undefined' && module.exports) {
    module.exports = CONFIG;
} else if (typeof self !== 'undefined') {
    self.CONFIG = CONFIG;
}
