#!/usr/bin/env bash
# Package the macOS release artefacts: sign the CLI binaries and DBCrust.app
# (hardened runtime), notarize and staple them, and build the DMG.
#
#   packaging/macos/package.sh --input DIR --out DIR [--version X.Y.Z]
#
# --input holds what the build jobs upload (either half may be missing):
#   macos-cli-<target>/{dbcrust,dbc,dbcrust-agent}   one directory per target
#   macos-gui-<target>/dbcrust-gui                   two targets make one universal app
# --out receives the release assets:
#   dbcrust-<target>.tar.gz + .sha256                the archives install.sh downloads
#   DBCrust-macos-universal.dmg + .sha256            DBCrust-macos-<arch>.dmg for a single slice
#
# Signing (environment):
#   MACOS_SIGNING_IDENTITY   a "Developer ID Application" identity (name or SHA-1 hash).
#                            Unset = ad-hoc signature (local builds, pull requests): the
#                            script never picks an identity from your keychains itself.
#   MACOS_KEYCHAIN           keychain holding that identity (CI: a temporary keychain)
# Notarization (needs a Developer ID identity):
#   APPLE_API_KEY_PATH, APPLE_API_KEY_ID, APPLE_API_ISSUER_ID
#                            App Store Connect API key (.p8 file, key ID, issuer ID)
#   NOTARY_PROFILE           or a profile saved by `xcrun notarytool store-credentials`
#   MACOS_REQUIRE_NOTARIZATION=1
#                            fail instead of producing un-notarized artefacts (releases)
#
# Runs on the bash 3.2 macOS ships: no empty "${array[@]}" under `set -u`.
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
abspath() { case "$1" in /*) printf '%s\n' "$1" ;; *) printf '%s/%s\n' "$PWD" "$1" ;; esac; }
die() { printf 'package.sh: %b\n' "$*" >&2; exit 1; }

version=""
input=""
out=""
while [ $# -gt 0 ]; do
    case "$1" in
        --input) input="$(abspath "$2")"; shift 2 ;;
        --out) out="$(abspath "$2")"; shift 2 ;;
        --version) version="$2"; shift 2 ;;
        -h | --help) sed -n '2,29p' "$0"; exit 0 ;;
        *) die "unknown argument $1 (see --help)" ;;
    esac
done
[ -n "$input" ] && [ -n "$out" ] || die "--input and --out are required (see --help)"
[ -d "$input" ] || die "$input is not a directory"
[ -n "$version" ] || version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -1)

# ── Signing and notarization settings ────────────────────────────────────────

identity="${MACOS_SIGNING_IDENTITY:--}"
keychain="${MACOS_KEYCHAIN:-}"

notary_mode=""
if [ -n "${APPLE_API_KEY_PATH:-}" ]; then
    [ -f "$APPLE_API_KEY_PATH" ] || die "APPLE_API_KEY_PATH: $APPLE_API_KEY_PATH does not exist"
    [ -n "${APPLE_API_KEY_ID:-}" ] || die "APPLE_API_KEY_ID is required with APPLE_API_KEY_PATH"
    notary_mode=key
elif [ -n "${NOTARY_PROFILE:-}" ]; then
    notary_mode=profile
fi

notarize=false
if [ "$identity" != "-" ] && [ -n "$notary_mode" ]; then
    notarize=true
elif [ -n "$notary_mode" ]; then
    echo "warning: notarization credentials given without MACOS_SIGNING_IDENTITY: an ad-hoc signature cannot be notarized" >&2
fi
if [ "${MACOS_REQUIRE_NOTARIZATION:-}" = 1 ] && [ "$notarize" != true ]; then
    [ "$identity" != "-" ] || die "MACOS_REQUIRE_NOTARIZATION=1 but MACOS_SIGNING_IDENTITY is not set"
    die "MACOS_REQUIRE_NOTARIZATION=1 but no notarization credentials (APPLE_API_KEY_PATH/APPLE_API_KEY_ID/APPLE_API_ISSUER_ID or NOTARY_PROFILE)"
fi

# codesign with the hardened runtime (required for notarization; an ad-hoc
# signature uses it too so local builds behave like releases). No
# entitlements: nothing in a release build loads third-party code at run time.
# The opt-in `duckdb` feature would (DuckDB extensions are dylibs) and need
# com.apple.security.cs.disable-library-validation.
sign() { # path [codesign options…]
    local path="$1"
    shift
    if [ "$identity" = "-" ]; then
        codesign --force --sign - --options runtime --timestamp=none "$@" "$path"
    elif [ -n "$keychain" ]; then
        codesign --force --sign "$identity" --keychain "$keychain" --options runtime --timestamp "$@" "$path"
    else
        codesign --force --sign "$identity" --options runtime --timestamp "$@" "$path"
    fi
    codesign --verify --strict --verbose=2 "$path"
}

notary() { # notarytool subcommand [arguments…]
    local command="$1"
    shift
    if [ "$notary_mode" = key ]; then
        xcrun notarytool "$command" "$@" --key "$APPLE_API_KEY_PATH" --key-id "$APPLE_API_KEY_ID" \
            ${APPLE_API_ISSUER_ID:+--issuer "$APPLE_API_ISSUER_ID"}
    else
        xcrun notarytool "$command" "$@" --keychain-profile "$NOTARY_PROFILE"
    fi
}

submit() { # file label
    local result="$work/notary-result.json" id status
    echo "Notarizing $2 (usually a few minutes)..."
    notary submit "$1" --wait --timeout 1h --output-format json >"$result" || true
    id=$(plutil -extract id raw -o - "$result" 2>/dev/null || true)
    status=$(plutil -extract status raw -o - "$result" 2>/dev/null || true)
    if [ "$status" != Accepted ]; then
        echo "error: notarization of $2 ended with status '${status:-none}'" >&2
        cat "$result" >&2
        if [ -n "$id" ]; then notary log "$id" >&2 || true; fi
        exit 1
    fi
    echo "Notarized $2 (submission $id)"
}

arch_of() { case "$1" in aarch64-*) echo arm64 ;; x86_64-*) echo x86_64 ;; esac; }

# A user's Mac has no Homebrew or MacPorts: only the OS's own libraries may be linked
check_binary() { # path arch…
    local path="$1" arch libs foreign
    shift
    for arch in "$@"; do
        lipo "$path" -verify_arch "$arch" || die "$path has no $arch slice (has: $(lipo -archs "$path"))"
    done
    # -arch all: plain `otool -L` reads only the host's slice of a universal binary
    libs=$(otool -arch all -L "$path") || die "otool cannot read $path"
    foreign=$(printf '%s\n' "$libs" | awk '/^\t/ { print $1 }' | grep -Ev '^(/usr/lib/|/System/Library/)' || true)
    [ -z "$foreign" ] || die "$path links libraries a user's Mac does not have:\n$foreign"
}

# ── Inputs ───────────────────────────────────────────────────────────────────

targets="aarch64-apple-darwin x86_64-apple-darwin"
cli_bins="dbcrust dbc dbcrust-agent"
cli_targets=""
gui_targets=""
for target in $targets; do
    if [ -d "$input/macos-cli-$target" ]; then cli_targets="$cli_targets $target"; fi
    if [ -f "$input/macos-gui-$target/dbcrust-gui" ]; then gui_targets="$gui_targets $target"; fi
done
[ -n "$cli_targets$gui_targets" ] || die "nothing to package in $input (expected macos-cli-<target>/ or macos-gui-<target>/)"

work=$(mktemp -d "${TMPDIR:-/tmp}/dbcrust-package.XXXXXX")
trap 'rm -rf "$work"' EXIT
mkdir -p "$out"

if [ "$identity" = "-" ]; then
    echo "Signing: ad-hoc (no MACOS_SIGNING_IDENTITY): fine for testing, Gatekeeper will block downloads"
else
    echo "Signing: $identity${keychain:+ (keychain $keychain)}; notarization: $notarize"
fi

# ── CLI binaries ─────────────────────────────────────────────────────────────

for target in $cli_targets; do
    mkdir -p "$work/cli/$target"
    for bin in $cli_bins; do
        src="$input/macos-cli-$target/$bin"
        [ -f "$src" ] || die "missing $src"
        install -m 0755 "$src" "$work/cli/$target/$bin"
        check_binary "$work/cli/$target/$bin" "$(arch_of "$target")"
        sign "$work/cli/$target/$bin" --identifier "com.dbcrust.$bin"
    done
done

# ── DBCrust.app ──────────────────────────────────────────────────────────────

app=""
dmg_name=""
if [ -n "$gui_targets" ]; then
    # shellcheck disable=SC2086 # a list of target triples, split on purpose
    set -- $gui_targets
    if [ $# -eq 2 ]; then
        lipo -create -output "$work/dbcrust-gui" \
            "$input/macos-gui-aarch64-apple-darwin/dbcrust-gui" \
            "$input/macos-gui-x86_64-apple-darwin/dbcrust-gui"
        check_binary "$work/dbcrust-gui" arm64 x86_64
        dmg_name="DBCrust-macos-universal.dmg"
    else
        install -m 0755 "$input/macos-gui-$1/dbcrust-gui" "$work/dbcrust-gui"
        check_binary "$work/dbcrust-gui" "$(arch_of "$1")"
        dmg_name="DBCrust-macos-${1%%-*}.dmg"
    fi
    bash "$root/gui/bundle/macos/bundle.sh" --binary "$work/dbcrust-gui" --out "$work/app" --version "$version"
    app="$work/app/DBCrust.app"
    sign "$app"
fi

# One submission covers the app and every CLI binary; only the app can carry
# its ticket (stapled): Gatekeeper looks bare executables up online.
if [ "$notarize" = true ]; then
    mkdir -p "$work/notarize"
    if [ -n "$app" ]; then ditto "$app" "$work/notarize/DBCrust.app"; fi
    for target in $cli_targets; do ditto "$work/cli/$target" "$work/notarize/$target"; done
    ditto -c -k --keepParent "$work/notarize" "$work/notarize.zip"
    submit "$work/notarize.zip" "the CLI binaries and DBCrust.app"
    if [ -n "$app" ]; then xcrun stapler staple "$app"; fi
fi

# ── Release assets ───────────────────────────────────────────────────────────

checksum() { (cd "$out" && shasum -a 256 "$1" >"$1.sha256"); }

for target in $cli_targets; do
    # COPYFILE_DISABLE: no AppleDouble (._*) entries for extended attributes
    # shellcheck disable=SC2086 # $cli_bins is a list of file names
    COPYFILE_DISABLE=1 tar -czf "$out/dbcrust-$target.tar.gz" -C "$work/cli/$target" $cli_bins
    checksum "dbcrust-$target.tar.gz"
done

if [ -n "$app" ]; then
    stage="$work/dmg"
    mkdir -p "$stage"
    ditto "$app" "$stage/DBCrust.app"
    ln -s /Applications "$stage/Applications"
    dmg="$out/$dmg_name"
    rm -f "$dmg"
    # hdiutil intermittently fails with "Resource busy" on CI runners
    attempt=1
    until hdiutil create -volname DBCrust -srcfolder "$stage" -fs HFS+ -format UDZO -ov "$dmg"; do
        [ "$attempt" -lt 5 ] || die "hdiutil create failed $attempt times"
        attempt=$((attempt + 1))
        sleep $((attempt * 5))
    done
    if [ "$identity" != "-" ]; then
        if [ -n "$keychain" ]; then
            codesign --force --sign "$identity" --keychain "$keychain" --timestamp "$dmg"
        else
            codesign --force --sign "$identity" --timestamp "$dmg"
        fi
    fi
    if [ "$notarize" = true ]; then
        submit "$dmg" "$dmg_name"
        xcrun stapler staple "$dmg"
    fi
    checksum "$dmg_name"
fi

# What a downloading user's Gatekeeper will decide
if [ "$notarize" = true ] && [ -n "$app" ]; then
    spctl --assess --type execute --verbose=2 "$app"
    spctl --assess --type open --context context:primary-signature --verbose=2 "$dmg"
    xcrun stapler validate "$dmg"
fi

if [ "$notarize" = true ]; then
    state="Developer ID signed and notarized"
elif [ "$identity" != "-" ]; then
    state="Developer ID signed, not notarized"
else
    state="ad-hoc signed (not for distribution)"
fi
assets=""
for asset in "$out"/*.tar.gz "$out"/*.dmg; do
    if [ -f "$asset" ]; then assets="$assets- ${asset##*/} ($(du -h "$asset" | cut -f1 | tr -d ' '))
"; fi
done
printf '\nmacOS packages (%s, %s):\n%s' "$version" "$state" "$assets"
if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
    printf '### macOS packages (%s)\n\n%s\n\n%s' "$version" "$state" "$assets" >>"$GITHUB_STEP_SUMMARY"
fi
