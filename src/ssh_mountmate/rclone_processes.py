from __future__ import annotations

import csv
import ctypes
import io
import json
import ntpath
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


def parse_windows_tasklist_process_ids(output: str) -> set[int]:
    process_ids: set[int] = set()
    for row in csv.reader(io.StringIO(output)):
        if len(row) < 2 or not is_windows_rclone_process_name(row[0]):
            continue
        try:
            process_ids.add(int(str(row[1]).replace(",", "")))
        except (TypeError, ValueError):
            continue
    return process_ids


def running_windows_rclone_process_paths_native() -> dict[int, str] | None:
    if os.name != "nt":
        return None
    try:
        capacity = 4096
        enum_processes = ctypes.windll.psapi.EnumProcesses
        enum_processes.argtypes = [ctypes.POINTER(ctypes.c_ulong), ctypes.c_ulong, ctypes.POINTER(ctypes.c_ulong)]
        while True:
            process_ids = (ctypes.c_ulong * capacity)()
            needed = ctypes.c_ulong()
            if not enum_processes(process_ids, ctypes.sizeof(process_ids), ctypes.byref(needed)):
                return None
            count = needed.value // ctypes.sizeof(ctypes.c_ulong)
            if count < capacity:
                break
            capacity *= 2
        matched: dict[int, str] = {}
        open_process = ctypes.windll.kernel32.OpenProcess
        open_process.argtypes = [ctypes.c_ulong, ctypes.c_int, ctypes.c_ulong]
        open_process.restype = ctypes.c_void_p
        query_image_name = ctypes.windll.kernel32.QueryFullProcessImageNameW
        query_image_name.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_wchar_p, ctypes.POINTER(ctypes.c_ulong)]
        close_handle = ctypes.windll.kernel32.CloseHandle
        close_handle.argtypes = [ctypes.c_void_p]
        for pid in process_ids[:count]:
            handle = open_process(0x1000, False, pid)
            if not handle:
                continue
            try:
                path = ctypes.create_unicode_buffer(32768)
                length = ctypes.c_ulong(len(path))
                if query_image_name(handle, 0, path, ctypes.byref(length)):
                    if is_windows_rclone_process_name(ntpath.basename(path.value)):
                        matched[int(pid)] = path.value
            finally:
                close_handle(handle)
        return matched
    except (AttributeError, OSError, ValueError):
        return None


def running_windows_rclone_process_ids_native() -> set[int] | None:
    paths = running_windows_rclone_process_paths_native()
    return None if paths is None else set(paths)


def running_windows_rclone_process_ids(timeout: float = 1.5) -> set[int]:
    native = running_windows_rclone_process_ids_native()
    if native is not None:
        return native
    try:
        result = subprocess.run(
            ["tasklist.exe", "/FO", "CSV", "/NH"],
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            creationflags=create_no_window(),
            timeout=timeout,
        )
    except Exception:
        return set()
    return parse_windows_tasklist_process_ids(result.stdout)


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
