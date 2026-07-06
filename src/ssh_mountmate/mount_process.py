from __future__ import annotations

import json
import os
import subprocess
import time
from pathlib import Path
from typing import Callable


def create_no_window() -> int:
    return getattr(subprocess, "CREATE_NO_WINDOW", 0) if os.name == "nt" else 0


def normalize_for_command(value: object) -> str:
    return str(value or "").strip().casefold()


def command_looks_like_rclone_mount(command: str) -> bool:
    text = command.casefold()
    return "rclone" in text and "mount" in text


def command_matches_state(command: str, state: dict, *, require_log: bool = True) -> bool:
    text = command.casefold()
    expected = [state.get("remote", ""), state.get("mountpoint", "")]
    if require_log:
        expected.append(state.get("log", ""))
    return all(normalize_for_command(value) in text for value in expected if value)


def command_matches_mount(command: str, state: dict) -> bool:
    if not command_looks_like_rclone_mount(command):
        return False
    return command_matches_state(command, state, require_log=False)


def running_rclone_processes() -> dict[int, str]:
    if os.name == "nt":
        return running_windows_rclone_processes()
    if Path("/proc").exists():
        return running_procfs_rclone_processes()
    return running_ps_rclone_processes()


def running_windows_rclone_processes() -> dict[int, str]:
    command = (
        "Get-CimInstance Win32_Process -Filter \"Name='rclone.exe'\" | "
        "Select-Object ProcessId,CommandLine | ConvertTo-Json -Compress"
    )
    try:
        result = subprocess.run(
            ["powershell.exe", "-NoProfile", "-Command", command],
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            creationflags=create_no_window(),
            timeout=5,
        )
    except Exception:
        return {}
    try:
        data = json.loads(result.stdout.strip() or "[]")
    except json.JSONDecodeError:
        return {}
    if isinstance(data, dict):
        data = [data]
    processes: dict[int, str] = {}
    for item in data:
        if not isinstance(item, dict):
            continue
        try:
            pid = int(item.get("ProcessId", 0))
        except (TypeError, ValueError):
            continue
        if pid:
            processes[pid] = str(item.get("CommandLine") or "")
    return processes


def running_procfs_rclone_processes(proc_root: Path = Path("/proc")) -> dict[int, str]:
    processes: dict[int, str] = {}
    for cmdline in proc_root.glob("[0-9]*/cmdline"):
        try:
            raw = cmdline.read_bytes()
        except OSError:
            continue
        if b"rclone" not in raw:
            continue
        try:
            pid = int(cmdline.parent.name)
        except ValueError:
            continue
        processes[pid] = raw.replace(b"\x00", b" ").decode(errors="ignore").strip()
    return processes


def running_ps_rclone_processes() -> dict[int, str]:
    try:
        result = subprocess.run(
            ["ps", "-axo", "pid=,command="],
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            timeout=5,
        )
    except Exception:
        return {}
    processes: dict[int, str] = {}
    for line in result.stdout.splitlines():
        stripped = line.strip()
        if not stripped:
            continue
        pid_text, _, command = stripped.partition(" ")
        if "rclone" not in command:
            continue
        try:
            processes[int(pid_text)] = command.strip()
        except ValueError:
            continue
    return processes


def process_command(pid: int) -> str:
    if not pid:
        return ""
    if os.name == "nt":
        return running_windows_rclone_processes().get(pid, "")
    proc_cmdline = Path("/proc") / str(pid) / "cmdline"
    if proc_cmdline.exists():
        try:
            raw = proc_cmdline.read_bytes()
        except OSError:
            raw = b""
        if raw:
            return raw.replace(b"\x00", b" ").decode(errors="ignore").strip()
    try:
        result = subprocess.run(
            ["ps", "-p", str(pid), "-o", "command="],
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            timeout=5,
        )
    except Exception:
        return ""
    return result.stdout.strip()


def pid_is_running(pid: int, pid_set: set[int] | None = None) -> bool:
    if pid_set is not None:
        return pid in pid_set
    if os.name == "nt":
        result = subprocess.run(
            ["tasklist", "/FI", f"PID eq {pid}", "/FO", "CSV", "/NH"],
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            creationflags=create_no_window(),
        )
        return str(pid) in result.stdout
    try:
        os.kill(pid, 0)
        return True
    except OSError:
        return False


def mount_status_from_state(
    state: dict,
    *,
    processes: dict[int, str] | None = None,
    mountpoint_ready: Callable[[str], bool] | None = None,
    allow_pid_fallback: bool = True,
) -> str:
    try:
        pid = int(state.get("pid", 0))
    except (TypeError, ValueError):
        return "stale"
    if not pid:
        return "stale"

    command = processes.get(pid, "") if processes is not None else process_command(pid)
    mountpoint = str(state.get("mountpoint") or "")
    ready = mountpoint_ready(mountpoint) if mountpoint and mountpoint_ready else None
    if command:
        if not command_matches_mount(command, state):
            return "stale"
        return "mounted" if ready is not False else "stale"

    if processes is not None and not allow_pid_fallback:
        return "stale"
    if allow_pid_fallback and pid_is_running(pid):
        return "mounted" if ready is not False else "stale"
    return "stale"


def process_matches_state_for_kill(pid: int, state: dict) -> bool:
    command = process_command(pid)
    return bool(command and command_matches_state(command, state, require_log=False))


def wait_for_mount_ready(
    proc: subprocess.Popen,
    mountpoint: str,
    log_path: Path,
    expected_state: dict,
    *,
    ready_before_start: bool,
    mountpoint_ready: Callable[[str], bool],
    timeout: float = 20.0,
    poll_interval: float = 0.25,
    stable_for: float = 0.75,
    process_command_func: Callable[[int], str] = process_command,
    sleep_func: Callable[[float], None] = time.sleep,
) -> None:
    deadline = time.time() + timeout
    ready_since = 0.0
    while time.time() < deadline:
        if proc.poll() is not None:
            break
        ready_now = mountpoint_ready(mountpoint)
        if ready_before_start:
            if not ready_now:
                ready_before_start = False
            sleep_func(poll_interval)
            continue
        if ready_now:
            if not ready_since:
                ready_since = time.time()
            if time.time() - ready_since >= stable_for:
                command = process_command_func(proc.pid)
                if command and not command_matches_mount(command, expected_state):
                    break
                return
        else:
            ready_since = 0.0
        sleep_func(poll_interval)
    if proc.poll() is None and ready_before_start:
        try:
            log_path.write_text(
                log_path.read_text(encoding="utf-8", errors="ignore")
                + f"\nMountpoint {mountpoint} already existed before this mount attempt.\n",
                encoding="utf-8",
            )
        except OSError:
            pass
    if proc.poll() is None:
        proc.terminate()
        sleep_func(0.5)
        if proc.poll() is None:
            proc.kill()
    tail = ""
    try:
        tail = "\n".join(log_path.read_text(encoding="utf-8", errors="ignore").splitlines()[-12:])
    except OSError:
        pass
    raise RuntimeError(f"Mount did not become ready. See log: {log_path}\n{tail}")
