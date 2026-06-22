//! RavenVault Linux companion app — library crate.
//!
//! This is the headless core of the RavenVault native app. The Chrome extension
//! (a WebSocket *client*) connects to the server this crate runs on
//! `127.0.0.1:53122`, ships raw Poe.com DOM HTML plus fetched asset bytes, and the
//! app converts that into clean Obsidian Markdown.
//!
//! See `docs/PROTOCOL.md` in the repository root for the full wire contract.

pub mod protocol;
pub mod server;

/// The version this app reports to the extension during the handshake.
///
/// The extension blocks all exports unless this is `>= MIN_APP_VERSION` (0.9.1)
/// as defined in the extension's `config.js`. Keep this in sync with the package
/// version in `Cargo.toml`.
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

/// The minimum extension version this app requires. Reported in the handshake as
/// `min_extension_version`; the extension marks itself outdated if older.
pub const MIN_EXTENSION_VERSION: &str = "0.10.0";

/// The protocol envelope version. Always the string "1".
pub const PROTOCOL_VERSION: &str = "1";

/// The loopback address the WebSocket server binds to. Fixed by the extension's
/// `config.js` (`ws://127.0.0.1:53122`).
pub const WS_BIND_ADDR: &str = "127.0.0.1:53122";

/// Raw chunk size (bytes, pre-base64) used by the extension for file transfers:
/// `1024 * 256` = 256 KiB. The app reassembles chunks of this size.
pub const CHUNK_SIZE: usize = 1024 * 256;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_version_is_at_least_min_app_version() {
        // The extension's MIN_APP_VERSION is "0.9.1"; we must report >= that.
        let min = "0.9.1";
        assert!(
            crate::cmp_versions(APP_VERSION, min) >= std::cmp::Ordering::Equal,
            "APP_VERSION {APP_VERSION} must be >= {min}"
        );
    }

    #[test]
    fn constants_match_extension_config() {
        assert_eq!(PROTOCOL_VERSION, "1");
        assert_eq!(WS_BIND_ADDR, "127.0.0.1:53122");
        assert_eq!(CHUNK_SIZE, 262_144);
    }
}

/// Compare two dotted numeric version strings (e.g. "0.10.0" vs "0.9.1").
///
/// Missing components are treated as 0, mirroring the extension's
/// `compareVersions` in `background.js`.
pub fn cmp_versions(a: &str, b: &str) -> std::cmp::Ordering {
    let parse = |s: &str| -> Vec<u64> {
        s.split('.')
            .map(|p| p.trim().parse::<u64>().unwrap_or(0))
            .collect()
    };
    let (va, vb) = (parse(a), parse(b));
    let n = va.len().max(vb.len());
    for i in 0..n {
        let x = va.get(i).copied().unwrap_or(0);
        let y = vb.get(i).copied().unwrap_or(0);
        match x.cmp(&y) {
            std::cmp::Ordering::Equal => continue,
            other => return other,
        }
    }
    std::cmp::Ordering::Equal
}

#[cfg(test)]
mod version_tests {
    use super::cmp_versions;
    use std::cmp::Ordering;

    #[test]
    fn semantic_numeric_comparison() {
        assert_eq!(cmp_versions("0.10.0", "0.9.1"), Ordering::Greater);
        assert_eq!(cmp_versions("0.9.1", "0.9.1"), Ordering::Equal);
        assert_eq!(cmp_versions("0.9.0", "0.9.1"), Ordering::Less);
        assert_eq!(cmp_versions("1", "0.9.9"), Ordering::Greater);
        assert_eq!(cmp_versions("0.9", "0.9.0"), Ordering::Equal);
    }
}
