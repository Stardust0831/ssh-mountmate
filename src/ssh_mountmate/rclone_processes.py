from __future__ import annotations

import json
import os
import re
import subprocess


MANAGED_RCLONE_EXE_RE = re.compile(r"^rclone-[0-9a-f]{16}\.exe$", re.IGNORECASE)


def create_no_window() -> int:
    return getattr(subprocess, "CREATE_NO_WINDOW", 0) if os.name == "nt" else 0


def is_windows_rclone_process_name(name: str) -> bool:
    normalized = str(name or "").strip()
    return normalized.casefold() == "rclone.exe" or bool(MANAGED_RCLONE_EXE_RE.fullmatch(normalized))


def parse_windows_rclone_processes(output: str) -> dict[int, str]:
    try:
        data = json.loads(output.strip() or "[]")
    except json.JSONDecodeError:
        return {}
    if isinstance(data, dict):
        data = [data]
    processes: dict[int, str] = {}
    for item in data:
        if not isinstance(item, dict):
            continue
        if not is_windows_rclone_process_name(str(item.get("Name") or "")):
            continue
        try:
            pid = int(item.get("ProcessId", 0))
        except (TypeError, ValueError):
            continue
        if pid:
            processes[pid] = str(item.get("CommandLine") or "")
    return processes


def running_windows_rclone_processes(timeout: float = 5) -> dict[int, str]:
    command = (
        "Get-CimInstance Win32_Process -Filter \"Name LIKE 'rclone%.exe'\" | "
        "Select-Object ProcessId,Name,CommandLine | ConvertTo-Json -Compress"
    )
    try:
        result = subprocess.run(
            ["powershell.exe", "-NoProfile", "-Command", command],
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            creationflags=create_no_window(),
            timeout=timeout,
        )
    except Exception:
        return {}
    return parse_windows_rclone_processes(result.stdout)


def running_windows_rclone_command_lines(timeout: float = 3) -> list[str]:
    return list(running_windows_rclone_processes(timeout=timeout).values())
