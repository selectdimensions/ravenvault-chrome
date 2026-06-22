#!/usr/bin/env bash
# Local CI gate for the RavenVault Linux app: format, lint, test.
# Run from the linux-app/ directory (or anywhere; it cd's to its own root).
set -euo pipefail

cd "$(dirname "$0")/.."

# Ensure the rust toolchain is on PATH even in non-login shells.
if [ -f "$HOME/.cargo/env" ]; then
    # shellcheck disable=SC1091
    . "$HOME/.cargo/env"
fi

echo "==> cargo fmt --check"
cargo fmt --all -- --check

echo "==> cargo clippy (warnings = errors)"
cargo clippy --all-targets --all-features -- -D warnings

echo "==> cargo test"
cargo test --all-features

echo "==> all checks passed"
