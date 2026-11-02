#!/bin/sh
# Builds Yatt-<version>.dmg: a window with Yatt.app and an Applications link
# (drag Yatt onto Applications). Signed ad hoc, so friends open it once via
# right-click > Open (see the "Как открыть" note inside the dmg).
#
#   scripts/make-dmg.sh   # -> target/release/Yatt-<version>.dmg
set -eu
cd "$(dirname "$0")/.."
# actool lives in full Xcode; use it even when xcode-select points at the CLT.
[ -d /Applications/Xcode.app ] && export DEVELOPER_DIR=${DEVELOPER_DIR:-/Applications/Xcode.app/Contents/Developer}

VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
APP=target/release/Yatt.app
DMG=target/release/Yatt-$VERSION.dmg
STAGE=target/release/dmg-stage

scripts/bundle-macos.sh
rm -rf "$DMG" "$STAGE"
mkdir -p "$STAGE"
cat > "$STAGE/Как открыть.txt" <<'TXT'
1. Перетащи Yatt на папку Applications.
2. Первый запуск: macOS скажет, что разработчик неизвестен (приложение не
   подписано платным Apple Developer ID). Это нормально.
   Открой Системные настройки > Конфиденциальность и безопасность,
   прокрути вниз и нажми «Всё равно открыть» рядом с Yatt.
   Либо в Терминале:  xattr -dr com.apple.quarantine /Applications/Yatt.app
3. Дальше Yatt открывается обычным двойным кликом.
Нужен macOS 14 или новее.
TXT
# dmgbuild writes .DS_Store directly (no Finder/AppleScript needed).
[ -x target/dmgenv/bin/dmgbuild ] || { python3 -m venv target/dmgenv && target/dmgenv/bin/pip install -q dmgbuild; }
APP="$APP" NOTE="$STAGE/Как открыть.txt" target/dmgenv/bin/dmgbuild -s assets/dmg/settings.py Yatt "$DMG"
rm -rf "$STAGE"
codesign --force --sign - "$DMG" 2>/dev/null || true
echo "built $DMG"
