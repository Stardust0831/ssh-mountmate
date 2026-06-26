#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
NAME="ssh-mountmate-windows"
TMP="$(mktemp -d)"
DIST="$TMP/$NAME"

cleanup() {
  rm -rf "$TMP"
}
trap cleanup EXIT

mkdir -p "$DIST/app" "$ROOT/dist"

cp "$ROOT/启动 SSH MountMate.cmd" "$DIST/启动 SSH MountMate.cmd"
cp "$ROOT/README.md" "$DIST/README.md"
cp "$ROOT/app/rsshmount.py" "$DIST/app/rsshmount.py"
cp "$ROOT/app/rsshmount_gui.pyw" "$DIST/app/rsshmount_gui.pyw"
cp "$ROOT/app/rsshmount.cmd" "$DIST/app/rsshmount.cmd"
cp "$ROOT/app/rsshmount-gui.cmd" "$DIST/app/rsshmount-gui.cmd"
cp "$ROOT/app/install.ps1" "$DIST/app/install.ps1"

python3 - "$ROOT/dist/$NAME.zip" "$DIST" "$NAME" <<'PY'
import os
import sys
import zipfile
from pathlib import Path

target = Path(sys.argv[1])
source = Path(sys.argv[2])
root_name = sys.argv[3]

with zipfile.ZipFile(target, "w", zipfile.ZIP_DEFLATED) as zf:
    for path in source.rglob("*"):
        if path.is_file():
            zf.write(path, Path(root_name) / path.relative_to(source))
print(target)
PY
