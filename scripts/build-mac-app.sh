#!/bin/bash
# Build Kani.app bundle for macOS
set -e

export PATH=/opt/homebrew/bin:$PATH
REPO_DIR="/Users/ramo/Projects/kani"
APP_DIR="$REPO_DIR/target/Kani.app"
CONTENTS="$APP_DIR/Contents"
MACOS="$CONTENTS/MacOS"
RESOURCES="$CONTENTS/Resources"
ASSETS="$REPO_DIR/crates/kani-gui/assets"

# Build release binary
echo "Building release binary..."
cd "$REPO_DIR"
cargo build -p kani-gui --release

# Create .app structure
echo "Creating app bundle..."
rm -rf "$APP_DIR"
mkdir -p "$MACOS" "$RESOURCES"

# Copy binary
cp "$REPO_DIR/target/release/kani-gui" "$MACOS/kani-gui"

# Generate .icns from PNG assets
echo "Generating app icon..."
ICONSET="$RESOURCES/app.iconset"
mkdir -p "$ICONSET"
cp "$ASSETS/app_16x16.png"   "$ICONSET/icon_16x16.png"
cp "$ASSETS/app_32x32.png"   "$ICONSET/icon_16x16@2x.png"
cp "$ASSETS/app_32x32.png"   "$ICONSET/icon_32x32.png"
cp "$ASSETS/app_64x64.png"   "$ICONSET/icon_32x32@2x.png"
cp "$ASSETS/app_128x128.png" "$ICONSET/icon_128x128.png"
cp "$ASSETS/app_256x256.png" "$ICONSET/icon_128x128@2x.png"
cp "$ASSETS/app_256x256.png" "$ICONSET/icon_256x256.png"
iconutil -c icns "$ICONSET" -o "$RESOURCES/app.icns"
rm -rf "$ICONSET"

# Write Info.plist
cat > "$CONTENTS/Info.plist" << 'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>
    <string>Kani</string>
    <key>CFBundleDisplayName</key>
    <string>Kani KVM</string>
    <key>CFBundleIdentifier</key>
    <string>com.kani.kvm</string>
    <key>CFBundleVersion</key>
    <string>0.1.0</string>
    <key>CFBundleShortVersionString</key>
    <string>0.1.0</string>
    <key>CFBundleExecutable</key>
    <string>kani-gui</string>
    <key>CFBundleIconFile</key>
    <string>app</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>NSHighResolutionCapable</key>
    <true/>
    <key>LSUIElement</key>
    <false/>
</dict>
</plist>
PLIST

# Ad-hoc code sign (required for macOS permission grants)
echo "Signing app bundle..."
codesign --force --deep -s - "$APP_DIR"

echo ""
echo "Done! App bundle created at: $APP_DIR"
echo ""
echo "To install:"
echo "  cp -r $APP_DIR /Applications/"
echo ""
echo "To run directly:"
echo "  open $APP_DIR"
