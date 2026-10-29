#!/usr/bin/env bash
# Build DBCrust.app (macOS) around the optimised desktop build.
#
#   mise run gui:bundle   →   target/gui-release/bundle/DBCrust.app (ad-hoc signed)
#
#   bundle.sh --binary PATH --out DIR [--version X.Y.Z]
#       wraps a prebuilt (possibly universal) binary instead of building one,
#       and leaves signing to the caller (packaging/macos/package.sh)
set -euo pipefail

root="$(cd "$(dirname "$0")/../../.." && pwd)"
# Arguments are relative to the caller's directory, not the repository's
abspath() { case "$1" in /*) printf '%s\n' "$1" ;; *) printf '%s/%s\n' "$PWD" "$1" ;; esac; }

version=""
binary=""
out="$root/target/gui-release/bundle"

while [ $# -gt 0 ]; do
    case "$1" in
        --binary) binary="$(abspath "$2")"; shift 2 ;;
        --out) out="$(abspath "$2")"; shift 2 ;;
        --version) version="$2"; shift 2 ;;
        *) echo "bundle.sh: unknown argument $1" >&2; exit 2 ;;
    esac
done

cd "$root"
# gui/ inherits the version from [workspace.package] in the root manifest
[ -n "$version" ] || version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)

sign_ad_hoc=false
if [ -z "$binary" ]; then
    cargo build -p dbcrust-gui --profile gui-release
    binary=target/gui-release/dbcrust-gui
    sign_ad_hoc=true
fi

app="$out/DBCrust.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
install -m 0755 "$binary" "$app/Contents/MacOS/DBCrust"
cp gui/bundle/macos/DBCrust.icns "$app/Contents/Resources/DBCrust.icns"
sed "s/__VERSION__/$version/g" gui/bundle/macos/Info.plist > "$app/Contents/Info.plist"
plutil -lint "$app/Contents/Info.plist" >/dev/null

if [ "$sign_ad_hoc" = true ]; then
    # Ad-hoc signature so Gatekeeper treats the bundle as one unit locally
    codesign --force --sign - "$app" >/dev/null 2>&1 || true
fi

echo "Built $app ($version)"
