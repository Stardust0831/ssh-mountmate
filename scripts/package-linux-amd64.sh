#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
NAME="rsshmount-linux-amd64"
TMP="$(mktemp -d)"
DIST="$TMP/$NAME"

cleanup() {
  rm -rf "$TMP"
}
trap cleanup EXIT

mkdir -p "$DIST/bin" "$ROOT/dist"

cp "$ROOT/app/rsshmount.py" "$DIST/rsshmount"
cp "$ROOT/README.md" "$DIST/README.md"
cp "$ROOT/install.sh" "$DIST/install.sh"
chmod +x "$DIST/rsshmount" "$DIST/install.sh"

curl -fsSL "https://downloads.rclone.org/rclone-current-linux-amd64.zip" -o "$TMP/rclone.zip"
unzip -q "$TMP/rclone.zip" -d "$TMP"
RCLONE_DIR="$(find "$TMP" -maxdepth 1 -type d -name 'rclone-*-linux-amd64' | head -n 1)"
test -n "$RCLONE_DIR"
cp "$RCLONE_DIR/rclone" "$DIST/bin/rclone"
chmod +x "$DIST/bin/rclone"

tar -C "$TMP" -czf "$ROOT/dist/$NAME.tar.gz" "$NAME"
echo "$ROOT/dist/$NAME.tar.gz"
