#!/bin/zsh
# tools/build.sh: builds the Swift helper and the Tauri app, signs them, and makes the installer
# dist/Clippy-<version>.dmg (drag Clippy to Applications).
#
# Needs: Xcode command line tools, Rust (rustup), the Tauri CLI (cargo install tauri-cli) and CMake
# (brew install cmake) for llama.cpp.
# Signing: set CLIPPY_SIGN_ID to a code-signing identity from your keychain; without it the app is
# signed ad hoc ("-"), which is fine for your own Mac.
# A personal/build.env file (not in Git) is read first if it exists, and personal/after-build.sh runs at the end.
set -e
cd "$(dirname $0)/.."
[[ -f personal/build.env ]] && source personal/build.env
export PATH="$HOME/.cargo/bin:$PATH"
if ! command -v cmake >/dev/null; then
  [[ -x "$HOME/.local/share/cmake-venv/bin/cmake" ]] && export PATH="$HOME/.local/share/cmake-venv/bin:$PATH"
fi
command -v cmake >/dev/null || { echo "CMake is needed to build llama.cpp: brew install cmake"; exit 1; }
export CMAKE="$(command -v cmake)"
VERSION=$(python3 -c "import json;print(json.load(open('src-tauri/tauri.conf.json'))['version'])")
APP="src-tauri/target/release/bundle/macos/Clippy.app"
ID="${CLIPPY_SIGN_ID:--}"

mkdir -p helper/build src-tauri/binaries dist
swiftc -O -o helper/build/clippy-helper helper/main.swift
cp helper/build/clippy-helper src-tauri/binaries/clippy-helper-aarch64-apple-darwin
[[ -n "$CLIPPY_NO_KILL" ]] || pkill -f "Clippy.app/Contents/MacOS" 2>/dev/null || true  # CLIPPY_NO_KILL=1 leaves a running copy alone
(cd src-tauri && cargo tauri build --bundles app)
codesign --force -s "$ID" "$APP/Contents/MacOS/clippy-helper"
codesign --force -s "$ID" "$APP"
codesign --verify --deep --strict "$APP" && echo "signed: $APP ($ID)"

# Installer: a compressed disk image with Clippy and a shortcut to Applications.
STAGE=$(mktemp -d)
cp -R "$APP" "$STAGE/"
# the copy in the DMG is signed with CLIPPY_DMG_SIGN_ID (a Developer ID if you have one), else ad hoc,
# so a personal local certificate never ends up inside a public download
DID="${CLIPPY_DMG_SIGN_ID:--}"
codesign --force -s "$DID" "$STAGE/Clippy.app/Contents/MacOS/clippy-helper"
codesign --force -s "$DID" "$STAGE/Clippy.app"
ln -s /Applications "$STAGE/Applications"
cat > "$STAGE/First open - read me.txt" <<TXT
Drag Clippy into Applications, then open it from there.

The first time, macOS may say it can't check the app for malware (it isn't notarised by Apple).
Right-click Clippy in Applications and choose Open, or go to System Settings → Privacy & Security
and click "Open Anyway". You only need to do this once.

Clippy lives in the menu bar (the paperclip). Its models run on your Mac; nothing you type leaves it.
TXT
DMG="dist/Clippy-$VERSION.dmg"
rm -f "$DMG"
hdiutil create -volname "Clippy $VERSION" -srcfolder "$STAGE" -ov -format UDZO "$DMG" >/dev/null
rm -rf "$STAGE"
codesign --force -s "$DID" "$DMG" 2>/dev/null || true
echo "installer: $DMG ($(du -h "$DMG" | cut -f1))"

[[ -x personal/after-build.sh ]] && personal/after-build.sh || true
