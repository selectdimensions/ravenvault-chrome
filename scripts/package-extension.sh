#!/usr/bin/env bash
# Build an upload-ready ZIP of the Chrome extension (only the files the browser
# needs — no linux-app/, docs/, git, or build artifacts). Output: dist/.
set -euo pipefail

cd "$(dirname "$0")/.."
ROOT="$(pwd)"

VERSION="$(grep -oE '"version"[[:space:]]*:[[:space:]]*"[^"]+"' manifest.json | head -1 | grep -oE '[0-9]+\.[0-9]+\.[0-9]+')"
OUT="dist/poe2obsidian-extension-${VERSION}.zip"

# The exact set of files the extension loads.
FILES=(
  manifest.json
  background.js
  content.js
  popup.html
  popup.js
  config.js
  ui-constants.js
  LICENSE
  icons
)

# Sanity: every listed path must exist.
for f in "${FILES[@]}"; do
  [ -e "$f" ] || { echo "ERROR: missing $f" >&2; exit 1; }
done

mkdir -p dist
rm -f "$OUT"
zip -r -X "$OUT" "${FILES[@]}" >/dev/null

echo "Built $OUT"
echo "Contents:"
unzip -l "$OUT" | awk 'NR>3 && NF>=4 {print "  "$4}' | sed '/^  $/d'
echo
echo "Upload this ZIP at https://chrome.google.com/webstore/devconsole (Unlisted)."
