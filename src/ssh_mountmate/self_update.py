from __future__ import annotations

import json
import os
import re
import shlex
import shutil
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Callable

from .paths import user_data_dir
from .updates import ReleaseAsset, download_verified_asset, running_onedir, safe_extract_zip


ProgressCallback = Callable[[str, int, int], None]


@dataclass(slots=True)
class InstallLayout:
    kind: str
    target: Path
    executable_relative: Path


@dataclass(slots=True)
class UpdatePlan:
    command: list[str]
    script: Path
    manifest: Path | None
    target: Path
    prepared: Path
    backup: Path


def safe_version_name(version: str) -> str:
    return re.sub(r"[^0-9A-Za-z._-]+", "-", str(version or "update")).strip(".-") or "update"


def cleanup_update_cache(*, max_age_seconds: float = 3600, now: float | None = None) -> None:
    root = user_data_dir() / "updates"
    if not root.is_dir():
        return
    cutoff = (time.time() if now is None else now) - max_age_seconds
    for child in root.iterdir():
        try:
            if child.stat().st_mtime > cutoff:
                continue
            if child.is_symlink() or child.is_file():
                child.unlink()
            elif child.is_dir():
                shutil.rmtree(child)
        except OSError:
            continue


def running_from_temporary_dir(executable: Path, *, temp_dir: Path | None = None, windows: bool | None = None) -> bool:
    is_windows = os.name == "nt" if windows is None else windows
    if not is_windows:
        return False
    try:
        return executable.resolve().is_relative_to((temp_dir or Path(tempfile.gettempdir())).resolve())
    except OSError:
        return False


def current_install_layout() -> InstallLayout:
    if not getattr(sys, "frozen", False):
        raise RuntimeError("Automatic installation is only available in packaged SSH MountMate builds.")
    executable = Path(sys.executable).resolve()
    if running_from_temporary_dir(executable):
        raise RuntimeError("SSH MountMate is running from a temporary ZIP extraction. Extract it to a permanent folder before using automatic updates.")
    if sys.platform == "darwin":
        for parent in executable.parents:
            if parent.suffix.casefold() == ".app":
                return InstallLayout("directory", parent, executable.relative_to(parent))
    if running_onedir():
        return InstallLayout("directory", executable.parent, Path(executable.name))
    return InstallLayout("file", executable, Path(executable.name))


def find_update_payload(extracted: Path, layout: InstallLayout) -> Path:
    if layout.kind == "file":
        candidates = sorted(
            (path for path in extracted.rglob(layout.target.name) if path.is_file()),
            key=lambda path: (len(path.relative_to(extracted).parts), str(path)),
        )
        if candidates:
            return candidates[0]
        raise RuntimeError(f"Update archive does not contain {layout.target.name}.")

    if layout.target.suffix.casefold() == ".app":
        candidates = sorted(
            (path for path in extracted.rglob("*.app") if (path / layout.executable_relative).is_file()),
            key=lambda path: (len(path.relative_to(extracted).parts), str(path)),
        )
    else:
        candidates = sorted(
            (
                path
                for path in extracted.rglob("*")
                if path.is_dir()
                and (path / layout.executable_relative).is_file()
                and (path / "_internal").is_dir()
            ),
            key=lambda path: (len(path.relative_to(extracted).parts), str(path)),
        )
    if not candidates:
        raise RuntimeError("Update archive does not contain a complete application directory.")
    return candidates[0]


def remove_prepared_path(path: Path) -> None:
    if not path.exists():
        return
    if path.is_dir():
        shutil.rmtree(path)
    else:
        path.unlink()


def stage_payload(payload: Path, layout: InstallLayout, version: str) -> tuple[Path, Path]:
    token = safe_version_name(version)
    prepared = layout.target.with_name(f".{layout.target.name}.update-{token}-new")
    backup = layout.target.with_name(f".{layout.target.name}.update-backup")
    if backup.exists():
        raise RuntimeError(f"A previous update backup still exists: {backup}. Restore or remove it before updating again.")
    remove_prepared_path(prepared)
    try:
        if layout.kind == "directory":
            shutil.copytree(payload, prepared)
        else:
            shutil.copy2(payload, prepared)
            if os.name != "nt":
                prepared.chmod(prepared.stat().st_mode | 0o700)
    except Exception:
        remove_prepared_path(prepared)
        raise
    staged_executable = prepared / layout.executable_relative if layout.kind == "directory" else prepared
    if not staged_executable.is_file():
        remove_prepared_path(prepared)
        raise RuntimeError("Prepared update is missing its executable.")
    return prepared, backup


WINDOWS_UPDATER = r'''param([Parameter(Mandatory=$true)][string]$Manifest)
$ErrorActionPreference = "Stop"
$cfg = Get-Content -LiteralPath $Manifest -Raw -Encoding UTF8 | ConvertFrom-Json
$log = [string]$cfg.log
function Write-UpdateLog([string]$message) {
    Add-Content -LiteralPath $log -Value ((Get-Date -Format o) + " " + $message)
}
function Move-WithRetry([string]$source, [string]$destination) {
    for ($attempt = 0; $attempt -lt 100; $attempt++) {
        try {
            Move-Item -LiteralPath $source -Destination $destination
            return
        } catch {
            if ($attempt -eq 99) { throw }
            Start-Sleep -Milliseconds 200
        }
    }
}
try {
    Wait-Process -Id ([int]$cfg.parent_pid) -ErrorAction SilentlyContinue
    if (Test-Path -LiteralPath $cfg.backup) { throw "Update backup already exists: $($cfg.backup)" }
    if (!(Test-Path -LiteralPath $cfg.prepared)) { throw "Prepared update is missing: $($cfg.prepared)" }
    Move-WithRetry $cfg.target $cfg.backup
    try {
        Move-WithRetry $cfg.prepared $cfg.target
        $launch = if ($cfg.kind -eq "directory") { Join-Path $cfg.target $cfg.executable_relative } else { $cfg.target }
        $process = Start-Process -FilePath $launch -WorkingDirectory (Split-Path -Parent $launch) -PassThru
        Start-Sleep -Seconds 5
        $process.Refresh()
        if ($process.HasExited) { throw "Updated application exited during startup." }
        Remove-Item -LiteralPath $cfg.backup -Recurse -Force
        Write-UpdateLog "Update completed successfully."
    } catch {
        Write-UpdateLog ("Updated application failed: " + $_.Exception.Message)
        if (Test-Path -LiteralPath $cfg.target) { Remove-Item -LiteralPath $cfg.target -Recurse -Force }
        if (Test-Path -LiteralPath $cfg.backup) { Move-Item -LiteralPath $cfg.backup -Destination $cfg.target }
        $rollbackLaunch = if ($cfg.kind -eq "directory") { Join-Path $cfg.target $cfg.executable_relative } else { $cfg.target }
        if (Test-Path -LiteralPath $rollbackLaunch) { Start-Process -FilePath $rollbackLaunch -WorkingDirectory (Split-Path -Parent $rollbackLaunch) }
        throw
    }
} catch {
    Write-UpdateLog ("Update failed: " + $_.Exception.Message)
    exit 1
}
'''


def create_windows_plan(layout: InstallLayout, prepared: Path, backup: Path, update_root: Path) -> UpdatePlan:
    update_root.mkdir(parents=True, exist_ok=True)
    script = update_root / "apply-update.ps1"
    manifest = update_root / "update-manifest.json"
    log = update_root / "update.log"
    script.write_text(WINDOWS_UPDATER, encoding="utf-8-sig")
    manifest.write_text(
        json.dumps(
            {
                "parent_pid": os.getpid(),
                "kind": layout.kind,
                "target": str(layout.target),
                "prepared": str(prepared),
                "backup": str(backup),
                "executable_relative": str(layout.executable_relative),
                "log": str(log),
            },
            indent=2,
        ),
        encoding="utf-8",
    )
    return UpdatePlan(
        command=["powershell.exe", "-NoProfile", "-ExecutionPolicy", "Bypass", "-File", str(script), str(manifest)],
        script=script,
        manifest=manifest,
        target=layout.target,
        prepared=prepared,
        backup=backup,
    )


def create_posix_plan(layout: InstallLayout, prepared: Path, backup: Path, update_root: Path) -> UpdatePlan:
    update_root.mkdir(parents=True, exist_ok=True)
    script = update_root / "apply-update.sh"
    log = update_root / "update.log"
    launch = layout.target / layout.executable_relative if layout.kind == "directory" else layout.target
    q = shlex.quote
    content = f'''#!/bin/sh
set -u
parent_pid={os.getpid()}
target={q(str(layout.target))}
prepared={q(str(prepared))}
backup={q(str(backup))}
launch={q(str(launch))}
log={q(str(log))}
while kill -0 "$parent_pid" 2>/dev/null; do sleep 0.2; done
if [ -e "$backup" ]; then printf '%s backup already exists\n' "$(date -Iseconds)" >> "$log"; exit 1; fi
if ! mv "$target" "$backup" || ! mv "$prepared" "$target"; then
  [ -e "$backup" ] && [ ! -e "$target" ] && mv "$backup" "$target"
  printf '%s replacement failed\n' "$(date -Iseconds)" >> "$log"
  exit 1
fi
"$launch" >/dev/null 2>&1 &
new_pid=$!
sleep 5
new_state=$(ps -p "$new_pid" -o stat= 2>/dev/null || true)
case "$new_state" in
  ""|*Z*) running=0 ;;
  *) running=1 ;;
esac
if [ "$running" -eq 1 ]; then
  rm -rf "$backup"
  printf '%s update completed successfully\n' "$(date -Iseconds)" >> "$log"
  exit 0
fi
rm -rf "$target"
mv "$backup" "$target"
"$launch" >/dev/null 2>&1 &
printf '%s updated application failed; rollback restored\n' "$(date -Iseconds)" >> "$log"
exit 1
'''
    script.write_text(content, encoding="utf-8")
    script.chmod(0o700)
    return UpdatePlan(
        command=["/bin/sh", str(script)],
        script=script,
        manifest=None,
        target=layout.target,
        prepared=prepared,
        backup=backup,
    )


def prepare_update(
    asset: ReleaseAsset,
    version: str,
    *,
    progress: ProgressCallback | None = None,
    update_root: Path | None = None,
) -> UpdatePlan:
    root = update_root or (user_data_dir() / "updates" / safe_version_name(version))
    root.mkdir(parents=True, exist_ok=True)
    archive = root / asset.name
    extracted = root / "extracted"
    if progress:
        progress("download", 0, asset.size)
    download_verified_asset(
        asset,
        archive,
        progress=(lambda current, total: progress("download", current, total)) if progress else None,
    )
    if progress:
        progress("extract", 0, 0)
    safe_extract_zip(archive, extracted)
    layout = current_install_layout()
    payload = find_update_payload(extracted, layout)
    if progress:
        progress("stage", 0, 0)
    prepared, backup = stage_payload(payload, layout, version)
    if os.name == "nt":
        return create_windows_plan(layout, prepared, backup, root)
    return create_posix_plan(layout, prepared, backup, root)


def launch_update(plan: UpdatePlan) -> None:
    kwargs = {
        "stdin": subprocess.DEVNULL,
        "stdout": subprocess.DEVNULL,
        "stderr": subprocess.DEVNULL,
        "cwd": str(plan.script.parent),
    }
    if os.name == "nt":
        kwargs["creationflags"] = getattr(subprocess, "CREATE_NO_WINDOW", 0) | getattr(subprocess, "CREATE_NEW_PROCESS_GROUP", 0)
    else:
        kwargs["start_new_session"] = True
    subprocess.Popen(plan.command, **kwargs)
    time.sleep(0.15)
