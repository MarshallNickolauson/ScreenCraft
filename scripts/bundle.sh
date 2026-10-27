#!/usr/bin/env bash
# Builds ScreenCraft in release mode and assembles a double-clickable ScreenCraft.app.
set -euo pipefail
cd "$(dirname "$0")/.."

# VP9 comes from libvpx. Link it statically when it is installed (brew install libvpx);
# otherwise build without it.
FEATURES=""
if pkg-config --exists vpx 2>/dev/null; then
  FEATURES="--features vp9"
  export VPX_STATIC=1
fi
cargo build --release -p screencraft $FEATURES

APP="dist/ScreenCraft.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp target/release/screencraft "$APP/Contents/MacOS/ScreenCraft"

# icon.icns from the 1024px master (see scripts/make-icon.swift)
ICONSET="$(mktemp -d)/ScreenCraft.iconset"
mkdir -p "$ICONSET"
MASTER=assets/icons/icon-1024.png
for s in 16 32 128 256 512; do
  sips -z $s $s "$MASTER" --out "$ICONSET/icon_${s}x${s}.png" >/dev/null
  sips -z $((s * 2)) $((s * 2)) "$MASTER" --out "$ICONSET/icon_${s}x${s}@2x.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/ScreenCraft.icns"

VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"

cat > "$APP/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>ScreenCraft</string>
  <key>CFBundleDisplayName</key><string>ScreenCraft</string>
  <key>CFBundleIdentifier</key><string>dev.screencraft.app</string>
  <key>CFBundleExecutable</key><string>ScreenCraft</string>
  <key>CFBundleIconFile</key><string>ScreenCraft</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>__VERSION__</string>
  <key>CFBundleVersion</key><string>1</string>
  <key>LSMinimumSystemVersion</key><string>15.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSCameraUseContinuityCameraDeviceType</key><true/>
  <key>NSCameraUsageDescription</key><string>ScreenCraft uses the camera to add your face to recordings.</string>
  <key>NSMicrophoneUsageDescription</key><string>ScreenCraft uses the microphone to record narration.</string>
  <key>NSScreenCaptureUsageDescription</key><string>ScreenCraft records your screen.</string>
</dict>
</plist>
PLIST
sed -i '' "s/__VERSION__/$VERSION/" "$APP/Contents/Info.plist"

# Sign with a *stable* designated requirement (bundle identifier only). A plain
# ad-hoc signature pins the code hash, so every rebuild looks like a brand-new
# app to macOS and the Screen Recording / Camera / Microphone grants are lost.
codesign --force --deep --sign - --identifier dev.screencraft.app \
  --requirements '=designated => identifier "dev.screencraft.app"' "$APP"
codesign -d -r- "$APP" 2>&1 | grep designated || true

if [ "${1:-}" = "--install" ]; then
  mkdir -p "$HOME/Applications"
  rm -rf "$HOME/Applications/ScreenCraft.app"
  cp -R "$APP" "$HOME/Applications/ScreenCraft.app"
  echo "Installed ~/Applications/ScreenCraft.app"
fi
echo "Built $APP"
