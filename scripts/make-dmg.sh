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
RW=target/release/Yatt-rw.dmg

scripts/bundle-macos.sh
rm -rf "$STAGE" "$DMG" "$RW"
mkdir -p "$STAGE/.background"
cp -R "$APP" "$STAGE/Yatt.app"
ln -s /Applications "$STAGE/Applications"
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

hdiutil create -srcfolder "$STAGE" -volname "Yatt" -fs HFS+ -format UDRW -ov "$RW" >/dev/null
DEV=$(hdiutil attach -readwrite -noverify -noautoopen "$RW" | awk '/Apple_HFS/ {print $1; exit}')
# Lay out the window: icons side by side, Applications on the right.
osascript <<OSA || echo "warning: Finder layout skipped"
tell application "Finder"
  tell disk "Yatt"
    open
    set current view of container window to icon view
    set toolbar visible of container window to false
    set statusbar visible of container window to false
    set bounds of container window to {200, 120, 760, 480}
    set opts to icon view options of container window
    set arrangement of opts to not arranged
    set icon size of opts to 112
    set position of item "Yatt.app" of container window to {150, 160}
    set position of item "Applications" of container window to {410, 160}
    set position of item "Как открыть.txt" of container window to {280, 300}
    close
    open
    update without registering applications
    delay 1
    close
  end tell
end tell
OSA
sync
hdiutil detach "$DEV" >/dev/null
hdiutil convert "$RW" -format UDZO -imagekey zlib-level=9 -o "$DMG" >/dev/null
rm -rf "$RW" "$STAGE"
codesign --force --sign - "$DMG" 2>/dev/null || true
echo "built $DMG"
