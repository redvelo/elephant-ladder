#!/usr/bin/env bash
# Packages the release build in src-tauri/target/release into a single-file
# Flatpak bundle. Run `node scripts/tauri-pinned.mjs build --no-bundle` first,
# on a host whose glibc is not newer than the runtime's (CI uses ubuntu-24.04).
#
# With FLATPAK_GPG_KEY_B64 set (CI), the OSTree repo is signed and exported to
# site/ for GitHub Pages, and the bundle embeds that repo as its origin, so a
# bundle install tracks later releases through `flatpak update`.
set -euo pipefail
cd "$(dirname "$0")/.."

APP_ID=com.redvelo.ElephantLadder
VERSION=$(node -p "require('./src-tauri/tauri.conf.json').version")
BUNDLE="elephant-ladder-${VERSION}-x86_64.flatpak"
REPO_URL=https://redvelo.github.io/elephant-ladder/repo

SIGN_ARGS=()
if [ -n "${FLATPAK_GPG_KEY_B64:-}" ]; then
    GNUPGHOME=$(mktemp -d)
    export GNUPGHOME
    printf '%s' "$FLATPAK_GPG_KEY_B64" | base64 -d | gpg --batch --quiet --import
    KEY_ID=$(gpg --list-secret-keys --with-colons | awk -F: '/^sec/ {print $5; exit}')
    SIGN_ARGS=("--gpg-sign=$KEY_ID" "--gpg-homedir=$GNUPGHOME")
fi

flatpak remote-add --user --if-not-exists flathub \
    https://flathub.org/repo/flathub.flatpakrepo
flatpak-builder --user --force-clean --install-deps-from=flathub \
    --repo=flatpak-repo "${SIGN_ARGS[@]}" flatpak-build "packaging/flatpak/${APP_ID}.yml"

if [ -n "${FLATPAK_GPG_KEY_B64:-}" ]; then
    flatpak build-update-repo "${SIGN_ARGS[@]}" flatpak-repo
    gpg --export "$KEY_ID" > repo-key.gpg
    rm -rf site
    mkdir -p site
    cp -r flatpak-repo site/repo
    # One-click remote setup: `flatpak remote-add --user elephant-ladder <this file's URL>`.
    cat > site/elephant-ladder.flatpakrepo <<EOF
[Flatpak Repo]
Title=Elephant Ladder
Url=$REPO_URL
Homepage=https://github.com/redvelo/elephant-ladder
GPGKey=$(base64 -w0 repo-key.gpg)
EOF
    flatpak build-bundle flatpak-repo "$BUNDLE" "$APP_ID" \
        --runtime-repo=https://flathub.org/repo/flathub.flatpakrepo \
        --repo-url="$REPO_URL" --gpg-keys=repo-key.gpg
    rm repo-key.gpg
else
    flatpak build-bundle flatpak-repo "$BUNDLE" "$APP_ID" \
        --runtime-repo=https://flathub.org/repo/flathub.flatpakrepo
fi

echo "wrote $BUNDLE"
