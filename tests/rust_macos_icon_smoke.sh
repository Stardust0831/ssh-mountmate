#!/usr/bin/env bash
set -euo pipefail

bundle="${1:?macOS application bundle is required}"
expected_icon="${2:-assets/ssh-mountmate-logo.icns}"
icon_name="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIconFile' "$bundle/Contents/Info.plist")"
test "$icon_name" = 'ssh-mountmate-logo.icns'
resource="$bundle/Contents/Resources/$icon_name"
test -s "$resource"
cmp "$expected_icon" "$resource"

temporary="$(mktemp -d "${TMPDIR:-/tmp}/ssh-mountmate-icon-XXXXXX")"
trap 'rm -rf "$temporary"' EXIT
iconutil --convert iconset --output "$temporary/logo.iconset" "$resource"
test -s "$temporary/logo.iconset/icon_16x16.png"
test -s "$temporary/logo.iconset/icon_512x512@2x.png"
printf 'macOS bundle icon verified: %s\n' "$resource"
