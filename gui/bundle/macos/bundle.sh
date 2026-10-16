#!/usr/bin/env bash
# Build DBCrust.app (macOS) from the optimised desktop build.
#   mise run gui:bundle   →   target/gui-release/bundle/DBCrust.app
set -euo pipefail

cd "$(dirname "$0")/../../.."
# gui/ inherits the version from [workspace.package] in the root manifest
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)

cargo build -p dbcrust-gui --profile gui-release

app=target/gui-release/bundle/DBCrust.app
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp target/gui-release/dbcrust-gui "$app/Contents/MacOS/DBCrust"
cp gui/bundle/macos/DBCrust.icns "$app/Contents/Resources/DBCrust.icns"
sed "s/__VERSION__/$version/g" gui/bundle/macos/Info.plist > "$app/Contents/Info.plist"

# Ad-hoc signature so Gatekeeper treats the bundle as one unit locally
codesign --force --deep --sign - "$app" >/dev/null 2>&1 || true

echo "Built $app ($version)"
