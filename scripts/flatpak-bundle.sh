#!/usr/bin/env bash
# Packages the release build in src-tauri/target/release into a single-file
# Flatpak bundle. Run `node scripts/tauri-pinned.mjs build --no-bundle` first,
# on a host whose glibc is not newer than the runtime's (CI uses ubuntu-24.04).
set -euo pipefail
cd "$(dirname "$0")/.."

APP_ID=com.redvelo.ElephantLadder
VERSION=$(node -p "require('./src-tauri/tauri.conf.json').version")
BUNDLE="elephant-ladder-${VERSION}-x86_64.flatpak"

flatpak remote-add --user --if-not-exists flathub \
    https://flathub.org/repo/flathub.flatpakrepo
flatpak-builder --user --force-clean --install-deps-from=flathub \
    --repo=flatpak-repo flatpak-build "packaging/flatpak/${APP_ID}.yml"
flatpak build-bundle flatpak-repo "$BUNDLE" "$APP_ID" \
    --runtime-repo=https://flathub.org/repo/flathub.flatpakrepo

echo "wrote $BUNDLE"
