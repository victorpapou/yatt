#!/bin/sh
# Builds Yatt.app: a release build in a bundle, signed ad hoc (no Developer ID
# needed; built here, it carries no quarantine, so it opens with a double-click).
#
#   scripts/bundle-macos.sh            # -> target/release/Yatt.app
#   scripts/bundle-macos.sh --install  # and copies it to /Applications
#
# The app keeps its data in ~/Library/Application Support/yatt (YATT_DB overrides).
set -eu
cd "$(dirname "$0")/.."

VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
APP=target/release/Yatt.app

cargo build --release -p yatt-app

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp target/release/yatt "$APP/Contents/MacOS/yatt"
# Liquid Glass icon (Icon Composer .icon); needs Xcode 26's actool.
xcrun actool assets/Yatt.icon --compile "$APP/Contents/Resources" \
    --output-format human-readable-text --notices --warnings --errors \
    --output-partial-info-plist target/release/icon-partial.plist \
    --app-icon Yatt --include-all-app-icons --enable-on-demand-resources NO \
    --development-region en --target-device mac --minimum-deployment-target 14.0 \
    --platform macosx

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleIdentifier</key><string>dev.yatt.app</string>
    <key>CFBundleName</key><string>Yatt</string>
    <key>CFBundleDisplayName</key><string>Yatt</string>
    <key>CFBundleIconFile</key><string>Yatt</string>
    <key>CFBundleIconName</key><string>Yatt</string>
    <key>CFBundleExecutable</key><string>yatt</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>$VERSION</string>
    <key>CFBundleVersion</key><string>$VERSION</string>
    <key>CFBundleDevelopmentRegion</key><string>ru</string>
    <key>LSMinimumSystemVersion</key><string>14.0</string>
    <key>LSApplicationCategoryType</key><string>public.app-category.productivity</string>
    <key>NSHighResolutionCapable</key><true/>
    <key>NSSupportsAutomaticGraphicsSwitching</key><true/>
</dict>
</plist>
PLIST

codesign --force --sign - --timestamp=none "$APP"
codesign --verify --strict "$APP"
echo "built $APP"

if [ "${1:-}" = "--install" ]; then
    pkill -x yatt 2>/dev/null && sleep 1 || true
    rm -rf /Applications/yatt.app /Applications/Yatt.app
    cp -R "$APP" /Applications/Yatt.app
    echo "installed /Applications/Yatt.app"
fi
