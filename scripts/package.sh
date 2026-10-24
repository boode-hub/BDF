#!/usr/bin/env bash
# Builds the portable folder: dist/BDF-<version>-<platform>/ and a zip of it.
# Usage: scripts/package.sh <platform> [target-dir]
set -euo pipefail
platform="${1:?platform}"
root="$(cd "$(dirname "$0")/.." && pwd)"
target="${2:-$root/target/release}"
version="$(grep -m1 '^version' "$root/Cargo.toml" | cut -d'"' -f2)"
name="BDF-${version}-${platform}"
out="$root/dist/$name"
rm -rf "$out" && mkdir -p "$out"
exe=bdf; [ -f "$target/bdf.exe" ] && exe=bdf.exe
cp "$target/$exe" "$out/"
cp "$root"/.pdfium/*pdfium* "$out/" 2>/dev/null || true
mkdir -p "$out/licenses"
cp "$root/.pdfium/PDFium-LICENSE.txt" "$out/licenses/"
cp "$root/crates/bdf/assets/Phosphor-LICENSE-MIT.txt" "$out/licenses/"
cp "$root/packaging/README.txt" "$out/"
mkdir -p "$out/BDF-data"   # presence = portable mode; settings stay in this folder
cd "$root/dist"
if command -v 7z >/dev/null 2>&1; then 7z a -bd "$name.zip" "$name" >/dev/null; else zip -qr "$name.zip" "$name"; fi
echo "dist/$name.zip"
