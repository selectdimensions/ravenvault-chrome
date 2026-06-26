#!/usr/bin/env bash
# Local CI gate for the Poe2Obsidian Linux app: format, lint, test.
# The headless core is always checked. The GUI crate (ravenvault-gui) is checked
# only when its system dependency (webkit2gtk-4.1) is available, so this script
# still passes on a headless machine.
set -euo pipefail

cd "$(dirname "$0")/.."

if [ -f "$HOME/.cargo/env" ]; then
    # shellcheck disable=SC1091
    . "$HOME/.cargo/env"
fi

echo "==> cargo fmt --check (all crates)"
cargo fmt --all -- --check

echo "==> cargo clippy — core (warnings = errors)"
cargo clippy -p ravenvault --all-targets --all-features -- -D warnings

echo "==> cargo test — core"
cargo test -p ravenvault --all-features

if pkg-config --exists webkit2gtk-4.1 2>/dev/null; then
    echo "==> cargo clippy — gui (webkit present)"
    cargo clippy -p ravenvault-gui --all-targets -- -D warnings
    echo "==> cargo build — gui"
    cargo build -p ravenvault-gui
else
    echo "==> skipping gui checks (webkit2gtk-4.1 not installed)"
fi

echo "==> all checks passed"
