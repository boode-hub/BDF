#!/usr/bin/env bash
# Downloads the pinned PDFium build (bblanchon/pdfium-binaries) into .pdfium/.
# Usage: scripts/fetch_pdfium.sh <platform>   e.g. win-x64, linux-x64, mac-arm64
set -euo pipefail
PDFIUM_BUILD="${PDFIUM_BUILD:-8076}"
platform="${1:?platform, e.g. win-x64}"
dest="$(cd "$(dirname "$0")/.." && pwd)/.pdfium"
mkdir -p "$dest"
url="https://github.com/bblanchon/pdfium-binaries/releases/download/chromium%2F${PDFIUM_BUILD}/pdfium-${platform}.tgz"
echo "Fetching $url"
tmp="$(mktemp -d)"
curl -fsSL --retry 4 --retry-delay 2 "$url" -o "$tmp/pdfium.tgz"
tar -xzf "$tmp/pdfium.tgz" -C "$tmp"
for f in "$tmp/bin/pdfium.dll" "$tmp/lib/libpdfium.so" "$tmp/lib/libpdfium.dylib"; do
  [ -f "$f" ] && cp "$f" "$dest/"
done
cp "$tmp/LICENSE" "$dest/PDFium-LICENSE.txt"
[ -f "$tmp/VERSION" ] && cp "$tmp/VERSION" "$dest/PDFium-VERSION.txt"
ls -la "$dest"
