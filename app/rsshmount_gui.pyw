#!/usr/bin/env python3
import argparse
import ctypes
import glob
import json
import locale
import os
import shutil
import socket
import subprocess
import sys
import threading
import time
from pathlib import Path
from tkinter import BOTH, END, LEFT, RIGHT, X, Y, BooleanVar, Button, Canvas, Checkbutton, Entry, Frame, Label, Scrollbar, StringVar, Text, Tk, Toplevel, filedialog, messagebox
from tkinter import font as tkfont
from tkinter import ttk

import rsshmount


APP_TITLE = "SSH MountMate"
CACHE_SIZE_CHOICES = ["default (off)", "1G", "5G", "10G", "20G", "50G", "100G", "500G"]
CACHE_AGE_CHOICES = ["default (1h0m0s)", "5m", "15m", "30m", "1h", "6h", "24h", "168h"]
MIN_FREE_CHOICES = ["default (off)", "1G", "5G", "10G", "20G", "50G", "100G"]
WRITE_BACK_CHOICES = ["default (5s)", "0s", "5s", "10s", "30s", "1m", "5m"]
DIR_CACHE_TIME_CHOICES = ["default (5m0s)", "30s", "1m", "5m", "15m", "1h"]
BUFFER_SIZE_CHOICES = ["default (16Mi)", "0", "8Mi", "16Mi", "32Mi", "64Mi", "128Mi"]
LANGUAGE_CHOICES = {"auto": "Auto", "en": "English", "zh": "中文"}
FONT_FAMILY_EN = "Segoe UI"
FONT_FAMILY_ZH = "Noto Sans CJK SC"
TEXT = {
    "en": {
        "ready": "Ready",
        "loading_configs": "Loading configs...",
        "no_configs": "No configs yet.",
        "settings": "Settings",
        "add_config": "Add config",
        "refresh": "Refresh",
        "checking_deps": "Checking dependencies...",
        "check_dependencies": "Check dependencies",
        "install_missing_dependencies": "Install missing dependencies",
        "missing_dependencies": "Missing dependencies: {items}. Install now?",
        "deps_status": "rclone: {rclone}    WinFsp: {winfsp}    ssh: {ssh}",
        "ok": "ok",
        "missing": "missing",
        "cache_root": "Cache root",
        "vfs_cache_mode": "VFS cache mode",
        "max_cache_size": "Max cache size",
        "max_cache_age": "Max cache age",
        "min_free_space": "Min free space",
        "write_back": "Write-back delay",
        "dir_cache_time": "Directory cache",
        "buffer_size": "Buffer size",
        "startup_all": "Mount all configs on Windows login",
        "language": "Language",
        "save_settings": "Save settings",
        "settings_saved": "Settings saved.",
        "installing_deps": "Installing missing dependencies...",
        "deps_complete": "Dependency check complete.",
        "deps_failed": "Dependency installation failed.",
        "mount": "Mount",
        "unmount": "Unmount",
        "open_folder": "Open mounted folder",
        "edit_mount": "Edit mount information",
        "edit_mounted_disabled": "Unmount before editing this config",
        "delete_config": "Delete this config",
        "refresh_remote": "Refresh remote directory cache",
        "refresh_unavailable": "Remount this config to enable refresh",
        "view_log": "View mount log",
        "copy": "Copy",
        "close": "Close",
        "error_details": "Error details",
        "mount_log": "Mount log",
        "copied": "Copied.",
        "remote_refreshed": "Remote directory cache refreshed.",
        "checking_capacity": "checking capacity",
        "unknown_capacity": "unknown capacity",
        "capacity_used": "{used} / {total} used ({percent}%)",
        "mounted_status": "mounted",
        "stopped_status": "stopped",
        "stale_status": "stale",
        "checking_status": "checking",
        "mounted_at": "Mounted {remote} at {mountpoint}",
        "unmounted": "Unmounted.",
        "mount_before_open": "Mount this config before opening its folder.",
        "delete_mounted_confirm": "{name} is mounted. Unmount and delete this config?",
        "delete_confirm": "Delete config {name}?",
        "deleted": "Deleted {name}.",
        "add_config_title": "Add config",
        "edit_config_title": "Edit config",
        "source": "Source",
        "ssh_config": "SSH config",
        "manual": "Manual",
        "ssh_host": "SSH Host",
        "name": "Name",
        "ip_host": "IP / Host",
        "user": "User",
        "port": "Port",
        "auth": "Auth",
        "key": "Key",
        "password_auth": "Password",
        "key_file": "Key file",
        "key_passphrase": "Key passphrase",
        "password": "Password",
        "remote_path": "Remote path",
        "mountpoint": "Mountpoint",
        "save": "Save",
        "cancel": "Cancel",
        "name_required": "Name is required.",
        "host_user_required": "IP/Host and user are required.",
        "password_required": "Password is required.",
    },
    "zh": {
        "ready": "就绪",
        "loading_configs": "正在加载配置...",
        "no_configs": "暂无配置。",
        "settings": "设置",
        "add_config": "新增配置",
        "refresh": "刷新",
        "checking_deps": "正在检查依赖...",
        "check_dependencies": "检查依赖",
        "install_missing_dependencies": "安装缺失依赖",
        "missing_dependencies": "缺少依赖：{items}。现在安装吗？",
        "deps_status": "rclone：{rclone}    WinFsp：{winfsp}    ssh：{ssh}",
        "ok": "正常",
        "missing": "缺失",
        "cache_root": "缓存目录",
        "vfs_cache_mode": "VFS 缓存模式",
        "max_cache_size": "最大缓存大小",
        "max_cache_age": "最大缓存寿命",
        "min_free_space": "最小剩余空间",
        "write_back": "写回延迟",
        "dir_cache_time": "目录缓存",
        "buffer_size": "读取缓冲",
        "startup_all": "Windows 登录时挂载全部配置",
        "language": "语言",
        "save_settings": "保存设置",
        "settings_saved": "设置已保存。",
        "installing_deps": "正在安装缺失依赖...",
        "deps_complete": "依赖检查完成。",
        "deps_failed": "依赖安装失败。",
        "mount": "挂载",
        "unmount": "取消挂载",
        "open_folder": "打开挂载目录",
        "edit_mount": "编辑挂载信息",
        "edit_mounted_disabled": "请先取消挂载，再编辑此配置",
        "delete_config": "删除此配置",
        "refresh_remote": "刷新远程目录缓存",
        "refresh_unavailable": "重新挂载后才能刷新缓存",
        "view_log": "查看挂载日志",
        "copy": "复制",
        "close": "关闭",
        "error_details": "错误详情",
        "mount_log": "挂载日志",
        "copied": "已复制。",
        "remote_refreshed": "远程目录缓存已刷新。",
        "checking_capacity": "正在检查容量",
        "unknown_capacity": "容量未知",
        "capacity_used": "已用 {used} / {total}（{percent}%）",
        "mounted_status": "已挂载",
        "stopped_status": "未挂载",
        "stale_status": "状态过期",
        "checking_status": "检查中",
        "mounted_at": "已挂载 {remote} 到 {mountpoint}",
        "unmounted": "已取消挂载。",
        "mount_before_open": "请先挂载此配置，再打开目录。",
        "delete_mounted_confirm": "{name} 正在挂载。是否取消挂载并删除此配置？",
        "delete_confirm": "删除配置 {name}？",
        "deleted": "已删除 {name}。",
        "add_config_title": "新增配置",
        "edit_config_title": "编辑配置",
        "source": "来源",
        "ssh_config": "SSH 配置",
        "manual": "手动",
        "ssh_host": "SSH Host",
        "name": "名称",
        "ip_host": "IP / 主机",
        "user": "用户名",
        "port": "端口",
        "auth": "认证",
        "key": "密钥",
        "password_auth": "密码",
        "key_file": "密钥文件",
        "key_passphrase": "密钥短语",
        "password": "密码",
        "remote_path": "远程路径",
        "mountpoint": "挂载点",
        "save": "保存",
        "cancel": "取消",
        "name_required": "名称必填。",
        "host_user_required": "IP/主机和用户名必填。",
        "password_required": "密码必填。",
    },
}
def app_dir() -> Path:
    return rsshmount.app_config_dir()


def servers_path() -> Path:
    return app_dir() / "servers.json"


def settings_path() -> Path:
    return app_dir() / "settings.json"


def default_settings() -> dict:
    return {
        "cache_root": str(rsshmount.xdg_cache_home()),
        "vfs_cache_mode": "writes",
        "vfs_cache_max_size": "",
        "vfs_cache_max_age": "",
        "vfs_cache_min_free_space": "",
        "vfs_write_back": "",
        "dir_cache_time": "",
        "buffer_size": "",
        "startup_all": False,
        "language": "auto",
    }


def load_settings() -> dict:
    settings = default_settings()
    path = settings_path()
    if path.exists():
        try:
            settings.update(json.loads(path.read_text(encoding="utf-8")))
        except Exception:
            pass
    return settings


def save_settings(settings: dict) -> None:
    app_dir().mkdir(parents=True, exist_ok=True)
    settings_path().write_text(json.dumps(settings, indent=2), encoding="utf-8")


def configured_cache_dir(host: str) -> Path:
    settings = load_settings()
    root = settings.get("cache_root") or default_settings()["cache_root"]
    return Path(root).expanduser() / host


def setting_to_choice(value: str, default_choice: str) -> str:
    return value if value else default_choice


def choice_to_setting(value: str) -> str:
    return "" if value.startswith("default ") else value


def system_language() -> str:
    try:
        lang = locale.getlocale()[0] or locale.getdefaultlocale()[0] or ""
    except Exception:
        lang = ""
    return "zh" if lang.lower().startswith("zh") else "en"


def effective_language(settings: dict | None = None) -> str:
    value = (settings or load_settings()).get("language", "auto")
    if value == "zh":
        return "zh"
    if value == "en":
        return "en"
    return system_language()


def tr_lang(lang: str, key: str, **kwargs) -> str:
    text = TEXT.get(lang, TEXT["en"]).get(key, TEXT["en"].get(key, key))
    return text.format(**kwargs) if kwargs else text


def language_choice_from_setting(value: str) -> str:
    return LANGUAGE_CHOICES.get(value or "auto", LANGUAGE_CHOICES["auto"])


def language_setting_from_choice(value: str) -> str:
    for key, label in LANGUAGE_CHOICES.items():
        if value == label:
            return key
    return "auto"


def bundled_dir() -> Path:
    if getattr(sys, "frozen", False):
        return Path(sys.executable).resolve().parent
    return Path(__file__).resolve().parent


def asset_dir() -> Path:
    return bundled_dir() / "assets"


def embedded_chinese_font() -> Path:
    return asset_dir() / "fonts" / "NotoSansCJKsc-Regular.otf"


def load_embedded_chinese_font() -> bool:
    font_path = embedded_chinese_font()
    if not font_path.exists() or os.name != "nt":
        return font_path.exists()
    try:
        return bool(ctypes.windll.gdi32.AddFontResourceExW(str(font_path), 0x10, 0))
    except Exception:
        return False


def configure_default_fonts(root: Tk, lang: str) -> None:
    family = FONT_FAMILY_ZH if lang == "zh" and load_embedded_chinese_font() else FONT_FAMILY_EN
    for name in ["TkDefaultFont", "TkTextFont", "TkMenuFont", "TkHeadingFont", "TkCaptionFont", "TkSmallCaptionFont"]:
        try:
            tkfont.nametofont(name).configure(family=family)
        except Exception:
            pass


def bundled_rclone_path() -> Path:
    return bundled_dir() / "bin" / ("rclone.exe" if os.name == "nt" else "rclone")


def refresh_windows_path_env() -> None:
    if os.name != "nt":
        return
    try:
        import winreg
    except ImportError:
        return

    registry_paths: list[str] = []
    keys = [
        (winreg.HKEY_CURRENT_USER, r"Environment"),
        (winreg.HKEY_LOCAL_MACHINE, r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment"),
    ]
    for root, subkey in keys:
        try:
            with winreg.OpenKey(root, subkey) as key:
                value, value_type = winreg.QueryValueEx(key, "Path")
        except OSError:
            continue
        if value_type == winreg.REG_EXPAND_SZ:
            value = winreg.ExpandEnvironmentStrings(value)
        registry_paths.extend(part for part in str(value).split(os.pathsep) if part)

    current_paths = [part for part in os.environ.get("PATH", "").split(os.pathsep) if part]
    merged: list[str] = []
    seen: set[str] = set()
    for part in current_paths + registry_paths:
        key = part.casefold()
        if key not in seen:
            seen.add(key)
            merged.append(part)
    if merged:
        os.environ["PATH"] = os.pathsep.join(merged)


def known_rclone_paths() -> list[Path]:
    if os.name != "nt":
        return []
    candidates: list[Path] = []
    localappdata = os.environ.get("LOCALAPPDATA")
    programfiles = os.environ.get("ProgramFiles")
    programfiles_x86 = os.environ.get("ProgramFiles(x86)")

    if localappdata:
        local = Path(localappdata)
        candidates.append(local / "Microsoft" / "WinGet" / "Links" / "rclone.exe")
        packages = local / "Microsoft" / "WinGet" / "Packages"
        if packages.exists():
            try:
                candidates.extend(packages.glob("Rclone.Rclone_*/**/rclone.exe"))
            except OSError:
                pass
    for root in [programfiles, programfiles_x86]:
        if root:
            candidates.append(Path(root) / "rclone" / "rclone.exe")
    return candidates


def resolve_rclone_path() -> str:
    bundled = bundled_rclone_path()
    if bundled.exists():
        return str(bundled)
    found = shutil.which("rclone.exe" if os.name == "nt" else "rclone") or shutil.which("rclone")
    if found:
        return found
    for candidate in known_rclone_paths():
        if candidate.exists():
            return str(candidate)
    return ""


def create_no_window() -> int:
    return getattr(subprocess, "CREATE_NO_WINDOW", 0) if os.name == "nt" else 0


def run(cmd: list[str], *, check=True, capture=False) -> subprocess.CompletedProcess:
    return subprocess.run(
        cmd,
        check=check,
        text=True,
        stdout=subprocess.PIPE if capture else None,
        stderr=subprocess.PIPE if capture else None,
        creationflags=create_no_window(),
    )


def free_local_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
        sock.bind(("127.0.0.1", 0))
        return int(sock.getsockname()[1])


def load_servers() -> list[dict]:
    path = servers_path()
    if not path.exists():
        return []
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except Exception:
        return []


def list_ssh_config_hosts(config_path: Path | None = None, seen: set[Path] | None = None) -> list[str]:
    config = config_path or (Path.home() / ".ssh" / "config")
    seen = seen or set()
    try:
        resolved = config.resolve()
    except OSError:
        resolved = config
    if resolved in seen or not config.exists():
        return []
    seen.add(resolved)

    hosts: list[str] = []
    for raw_line in config.read_text(encoding="utf-8", errors="ignore").splitlines():
        line = raw_line.strip()
        if not line or line.startswith("#"):
            continue
        words = line.split()
        if not words:
            continue
        keyword = words[0].lower()
        if keyword == "include":
            for pattern in words[1:]:
                expanded = os.path.expanduser(pattern)
                if not os.path.isabs(expanded):
                    expanded = str(config.parent / expanded)
                for included in glob.glob(expanded):
                    hosts.extend(list_ssh_config_hosts(Path(included), seen))
        elif keyword == "host":
            for host in words[1:]:
                if "*" not in host and "?" not in host and "!" not in host:
                    hosts.append(host)

    unique: list[str] = []
    for host in hosts:
        if host not in unique:
            unique.append(host)
    return unique


def save_servers(servers: list[dict]) -> None:
    app_dir().mkdir(parents=True, exist_ok=True)
    servers_path().write_text(json.dumps(servers, indent=2), encoding="utf-8")


def server_label(server: dict) -> str:
    name = server.get("name") or server.get("id")
    mode = server.get("source") or server.get("mode", "")
    mountpoint = server.get("mountpoint") or "Auto"
    status = mount_status(server)
    return f"{name}  [{mode}]  {mountpoint}  - {status}"


def server_state_file(server: dict) -> Path:
    return rsshmount.app_state_dir() / f"{server['id']}.json"


def running_pid_set() -> set[int]:
    if os.name != "nt":
        return set()
    result = subprocess.run(
        ["tasklist", "/FI", "IMAGENAME eq rclone.exe", "/FO", "CSV", "/NH"],
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
        creationflags=create_no_window(),
    )
    pids: set[int] = set()
    for line in result.stdout.splitlines():
        parts = [part.strip().strip('"') for part in line.split(",")]
        if len(parts) > 1 and parts[1].isdigit():
            pids.add(int(parts[1]))
    return pids


def running_rclone_processes() -> dict[int, str]:
    if os.name != "nt":
        return {}
    command = (
        "Get-CimInstance Win32_Process -Filter \"Name='rclone.exe'\" | "
        "Select-Object ProcessId,CommandLine | ConvertTo-Json -Compress"
    )
    result = subprocess.run(
        ["powershell.exe", "-NoProfile", "-Command", command],
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
        creationflags=create_no_window(),
    )
    try:
        data = json.loads(result.stdout.strip() or "[]")
    except json.JSONDecodeError:
        return {}
    if isinstance(data, dict):
        data = [data]
    processes: dict[int, str] = {}
    for item in data:
        try:
            pid = int(item.get("ProcessId", 0))
        except (TypeError, ValueError):
            continue
        if pid:
            processes[pid] = str(item.get("CommandLine") or "")
    return processes


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


def mount_status(server: dict) -> str:
    state_file = server_state_file(server)
    if not state_file.exists():
        return "stopped"
    try:
        state = json.loads(state_file.read_text(encoding="utf-8"))
        pid = int(state.get("pid", 0))
    except Exception:
        return "stale"
    return "mounted" if pid and pid_is_running(pid) else "stale"


def mount_status_with_pids(server: dict, pid_set: set[int]) -> str:
    state_file = server_state_file(server)
    if not state_file.exists():
        return "stopped"
    try:
        state = json.loads(state_file.read_text(encoding="utf-8"))
        pid = int(state.get("pid", 0))
    except Exception:
        return "stale"
    return "mounted" if pid and pid_is_running(pid, pid_set) else "stale"


def command_matches_state(command: str, state: dict) -> bool:
    command = command.casefold()
    expected = [state.get("remote", ""), state.get("mountpoint", ""), state.get("log", "")]
    return all(str(value).casefold() in command for value in expected if value)


def mount_status_with_processes(server: dict, processes: dict[int, str]) -> str:
    state_file = server_state_file(server)
    if not state_file.exists():
        return "stopped"
    try:
        state = json.loads(state_file.read_text(encoding="utf-8"))
        pid = int(state.get("pid", 0))
    except Exception:
        return "stale"
    command = processes.get(pid)
    if not pid or not command:
        return "stale"
    return "mounted" if command_matches_state(command, state) else "stale"


def verified_mount_status(server: dict) -> str:
    if os.name == "nt":
        return mount_status_with_processes(server, running_rclone_processes())
    return mount_status(server)


def mountpoint_ready(mountpoint: str) -> bool:
    try:
        if os.name == "nt" and len(mountpoint) in (2, 3) and mountpoint[1] == ":":
            return Path(mountpoint[:2] + "\\").exists()
        return Path(mountpoint).exists()
    except OSError:
        return False


def wait_for_mount_ready(proc: subprocess.Popen, mountpoint: str, log_path: Path, timeout: float = 20.0) -> None:
    deadline = time.time() + timeout
    while time.time() < deadline:
        if proc.poll() is not None:
            break
        if mountpoint_ready(mountpoint):
            return
        time.sleep(0.25)
    if proc.poll() is None:
        proc.terminate()
        time.sleep(0.5)
        if proc.poll() is None:
            proc.kill()
    tail = ""
    try:
        tail = "\n".join(log_path.read_text(encoding="utf-8", errors="ignore").splitlines()[-12:])
    except OSError:
        pass
    raise RuntimeError(f"Mount did not become ready. See log: {log_path}\n{tail}")


def current_mountpoint(server: dict) -> str:
    state_file = server_state_file(server)
    if state_file.exists():
        try:
            state = json.loads(state_file.read_text(encoding="utf-8"))
            if state.get("mountpoint"):
                return state["mountpoint"]
        except Exception:
            pass
    configured = server.get("mountpoint") or ""
    if configured and configured.lower() != "auto":
        return configured
    return str(rsshmount.default_mountpoint(remote_name(server)))


def current_state(server: dict) -> dict:
    state_file = server_state_file(server)
    if not state_file.exists():
        return {}
    try:
        return json.loads(state_file.read_text(encoding="utf-8"))
    except Exception:
        return {}


def current_log_path(server: dict) -> Path:
    state = current_state(server)
    if state.get("log"):
        return Path(state["log"])
    return rsshmount.app_state_dir() / f"{remote_name(server)}.log"


def display_mountpoint(server: dict) -> str:
    mountpoint = current_mountpoint(server)
    return mountpoint if mountpoint else "Auto"


def format_capacity_bytes(size: int) -> str:
    units = [("TB", 1000**4), ("GB", 1000**3), ("MB", 1000**2), ("KB", 1000)]
    for unit, factor in units:
        if abs(size) >= factor:
            return f"{size / factor:.1f} {unit}"
    return f"{size} B"


def capacity_info(server: dict, status: str | None = None) -> dict:
    if (status or verified_mount_status(server)) != "mounted":
        return {}
    mountpoint = current_mountpoint(server)
    try:
        usage = shutil.disk_usage(mountpoint)
    except OSError:
        return {}
    used = max(usage.total - usage.free, 0)
    percent = int(round((used / usage.total) * 100)) if usage.total else 0
    return {"used": used, "total": usage.total, "percent": max(0, min(percent, 100))}


def split_remote_path(remote_path: str) -> tuple[str, str]:
    path = (remote_path or "").strip()
    if not path or path == "~":
        return "$HOME", ""
    if path.startswith("/"):
        return "/", path[1:]
    return "$HOME", path


def compose_remote_path(base: str, suffix: str) -> str:
    suffix = (suffix or "").strip().replace("\\", "/").strip("/")
    if base == "/":
        return "/" + suffix if suffix else "/"
    return suffix


def mountpoint_choices() -> list[str]:
    if os.name != "nt":
        return ["Auto"]
    choices = ["Auto"]
    for letter in "ZYXWVUTSRQPONMLKJIHGFED":
        drive = f"{letter}:"
        if not Path(drive + "\\").exists():
            choices.append(drive)
    return choices


def ssh_config_defaults(host_alias: str) -> dict:
    if not host_alias:
        return {}
    config = rsshmount.read_ssh_config(host_alias, None)
    key_file = rsshmount.first_usable_path(config.get("identityfile", []), must_exist=True)
    return {
        "name": host_alias,
        "host_alias": host_alias,
        "host": rsshmount.first_ssh_value(config, "hostname", host_alias),
        "user": rsshmount.first_ssh_value(config, "user", ""),
        "port": rsshmount.first_ssh_value(config, "port", "22"),
        "key_file": key_file,
    }


def winfsp_installed() -> bool:
    return rsshmount.find_winfsp() is not None


def ssh_installed() -> bool:
    return shutil.which("ssh") is not None


def ps_quote(value: str) -> str:
    return "'" + value.replace("'", "''") + "'"


def run_visible_winget_install(title: str, package_id: str) -> tuple[int, Path]:
    app_dir().mkdir(parents=True, exist_ok=True)
    script = app_dir() / f"install-{package_id.replace('.', '-')}.cmd"
    ps_script = app_dir() / f"install-{package_id.replace('.', '-')}.ps1"
    log_path = app_dir() / f"install-{package_id.replace('.', '-')}.log"
    log_path.write_text(
        "\n".join(
            [
                "==== SSH MountMate installer launcher ====",
                f"Package: {package_id}",
                f"Batch script: {script}",
                f"PowerShell script: {ps_script}",
                "",
            ]
        ),
        encoding="utf-8",
    )
    ps_script.write_text(
        "\n".join(
            [
                "$ErrorActionPreference = 'Continue'",
                f"$LogPath = {ps_quote(str(log_path))}",
                "New-Item -ItemType Directory -Force -Path (Split-Path -Parent $LogPath) | Out-Null",
                'function Write-InstallLog([string]$Message) { $Message | Tee-Object -FilePath $LogPath -Append }',
                'Write-InstallLog ""',
                'Write-InstallLog "==== SSH MountMate dependency install ===="',
                'Write-InstallLog ("Started: " + (Get-Date -Format o))',
                f'Write-InstallLog "Package: {package_id}"',
                'Write-InstallLog ("Log: " + $LogPath)',
                "$Winget = Get-Command winget.exe -ErrorAction SilentlyContinue",
                "if (-not $Winget) {",
                '  Write-InstallLog "winget.exe was not found in PATH."',
                '  Write-InstallLog "Install App Installer from Microsoft Store, then retry."',
                '  Write-InstallLog ""',
                "  exit 9009",
                "}",
                'Write-InstallLog ("winget: " + $Winget.Source)',
                'Write-InstallLog ""',
                f'& winget install --id "{package_id}" -e --accept-package-agreements --accept-source-agreements 2>&1 | Tee-Object -FilePath $LogPath -Append',
                "$RC = $LASTEXITCODE",
                'Write-InstallLog ""',
                'Write-InstallLog ("Finished: " + (Get-Date -Format o))',
                'Write-InstallLog ("Exit code: " + $RC)',
                "if ($RC -eq 0) {",
                '  Write-InstallLog "Installation command completed successfully."',
                "  exit 0",
                "}",
                'Write-InstallLog "Installation failed."',
                "exit $RC",
            ]
        ),
        encoding="utf-8",
    )
    script.write_text(
        "\n".join(
            [
                "@echo off",
                f"title SSH MountMate - {title}",
                f'>> "{log_path}" echo ==== SSH MountMate installer wrapper ====',
                f'>> "{log_path}" echo Started: %DATE% %TIME%',
                f'>> "{log_path}" echo Package: {package_id}',
                f'>> "{log_path}" echo PowerShell script: "{ps_script}"',
                f'>> "{log_path}" echo.',
                f'echo Log file: "{log_path}"',
                "echo.",
                f'where powershell.exe >> "{log_path}" 2>&1',
                f'>> "{log_path}" echo Launching PowerShell installer...',
                f'powershell.exe -NoProfile -ExecutionPolicy Bypass -File "{ps_script}"',
                "set RC=%ERRORLEVEL%",
                f'>> "{log_path}" echo Wrapper exit code: %RC%',
                'if "%RC%"=="0" (',
                "  echo.",
                "  echo Installation command completed. This window will close in 5 seconds...",
                f'  timeout /t 5 /nobreak >> "{log_path}"',
                "  exit 0",
                ")",
                "echo.",
                "echo Installation failed or could not start. Exit code: %RC%",
                f'echo Log file: "{log_path}"',
                "echo This command window is intentionally left open for troubleshooting.",
            ]
        ),
        encoding="utf-8",
    )
    result = subprocess.run(
        ["cmd.exe", "/k", str(script)],
        creationflags=getattr(subprocess, "CREATE_NEW_CONSOLE", 0),
        check=False,
    )
    return result.returncode, log_path


def install_rclone() -> None:
    if resolve_rclone_path():
        return
    if os.name != "nt":
        raise RuntimeError("rclone is missing. Install rclone and retry.")
    code, log_path = run_visible_winget_install("rclone", "Rclone.Rclone")
    refresh_windows_path_env()
    if resolve_rclone_path():
        return
    raise RuntimeError(f"rclone was not found after winget finished. winget exit code: {code}. Log: {log_path}")


def install_winfsp() -> None:
    if winfsp_installed():
        return
    if os.name != "nt":
        raise RuntimeError("WinFsp is only required on Windows.")
    code, log_path = run_visible_winget_install("WinFsp", "WinFsp.WinFsp")
    refresh_windows_path_env()
    if winfsp_installed():
        return
    raise RuntimeError(f"WinFsp was not found after winget finished. winget exit code: {code}. Log: {log_path}")


def install_openssh_client() -> None:
    command = (
        "Start-Process powershell -Verb RunAs "
        "-ArgumentList '-NoProfile -ExecutionPolicy Bypass -Command "
        "\"Add-WindowsCapability -Online -Name OpenSSH.Client~~~~0.0.1.0\"'"
    )
    run(["powershell.exe", "-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", command])


def obscure_password(rclone: str, password: str) -> str:
    result = run([rclone, "obscure", password], capture=True)
    return result.stdout.strip()


def write_manual_remote(server: dict, rclone: str) -> None:
    import configparser

    conf_path = rsshmount.rclone_config_path()
    conf_path.parent.mkdir(parents=True, exist_ok=True)
    parser = configparser.RawConfigParser()
    parser.optionxform = str
    parser.read(conf_path)

    remote = server["id"]
    if parser.has_section(remote):
        parser.remove_section(remote)
    parser.add_section(remote)
    parser.set(remote, "type", "sftp")
    parser.set(remote, "host", server["host"])
    parser.set(remote, "user", server["user"])
    parser.set(remote, "port", str(server.get("port") or "22"))
    parser.set(remote, "shell_type", "unix")
    parser.set(remote, "disable_hashcheck", "true")

    if server.get("auth") == "password":
        parser.set(remote, "pass", server["password_obscured"])
    elif server.get("key_file"):
        parser.set(remote, "key_file", server["key_file"])
        if server.get("key_pass_obscured"):
            parser.set(remote, "key_file_pass", server["key_pass_obscured"])
    else:
        parser.set(remote, "key_use_agent", "true")

    known_hosts = rsshmount.default_known_hosts_file()
    if known_hosts.exists():
        parser.set(remote, "known_hosts_file", str(known_hosts))

    with conf_path.open("w", encoding="utf-8") as fh:
        parser.write(fh)


def ensure_remote(server: dict, rclone: str) -> None:
    if server["mode"] == "ssh_config":
        rsshmount.ensure_rclone_remote(server["host_alias"], None, "auto")
    else:
        write_manual_remote(server, rclone)


def remote_name(server: dict) -> str:
    return server["host_alias"] if server["mode"] == "ssh_config" else server["id"]


def mount_server(server: dict, rclone: str) -> dict:
    if verified_mount_status(server) == "mounted":
        raise RuntimeError("This config is already mounted. Unmount it before mounting again.")
    ensure_remote(server, rclone)
    settings = load_settings()
    state_dir = rsshmount.app_state_dir()
    cache_dir = configured_cache_dir(remote_name(server))
    state_dir.mkdir(parents=True, exist_ok=True)
    cache_dir.mkdir(parents=True, exist_ok=True)

    remote_path = server.get("remote_path") or ""
    configured_mountpoint = server.get("mountpoint") or ""
    mountpoint = (
        str(rsshmount.default_mountpoint(remote_name(server)))
        if not configured_mountpoint or configured_mountpoint.lower() == "auto"
        else configured_mountpoint
    )
    remote = rsshmount.remote_spec(remote_name(server), remote_path)
    log_path = state_dir / f"{remote_name(server)}.log"
    rc_addr = f"127.0.0.1:{free_local_port()}"

    cmd = [
        rclone,
        "--config",
        str(rsshmount.rclone_config_path()),
        "--rc",
        "--rc-no-auth",
        "--rc-addr",
        rc_addr,
        "mount",
        remote,
        mountpoint,
        "--vfs-cache-mode",
        server.get("cache_mode") or settings.get("vfs_cache_mode", "writes"),
        "--vfs-fast-fingerprint",
        "--cache-dir",
        str(cache_dir),
        "--log-file",
        str(log_path),
        "--volname",
        server.get("name") or remote_name(server),
    ]
    if settings.get("vfs_cache_max_size"):
        cmd.extend(["--vfs-cache-max-size", settings["vfs_cache_max_size"]])
    if settings.get("vfs_cache_max_age"):
        cmd.extend(["--vfs-cache-max-age", settings["vfs_cache_max_age"]])
    if settings.get("vfs_cache_min_free_space"):
        cmd.extend(["--vfs-cache-min-free-space", settings["vfs_cache_min_free_space"]])
    if settings.get("vfs_write_back"):
        cmd.extend(["--vfs-write-back", settings["vfs_write_back"]])
    if settings.get("dir_cache_time"):
        cmd.extend(["--dir-cache-time", settings["dir_cache_time"]])
    if settings.get("buffer_size"):
        cmd.extend(["--buffer-size", settings["buffer_size"]])
    if server.get("network_mode"):
        cmd.append("--network-mode")

    log = log_path.open("ab")
    flags = (
        getattr(subprocess, "DETACHED_PROCESS", 0)
        | getattr(subprocess, "CREATE_NEW_PROCESS_GROUP", 0)
        | getattr(subprocess, "CREATE_NO_WINDOW", 0)
    )
    proc = subprocess.Popen(cmd, stdout=log, stderr=subprocess.STDOUT, creationflags=flags)
    wait_for_mount_ready(proc, mountpoint, log_path)
    state = {"pid": proc.pid, "server_id": server["id"], "remote": remote, "mountpoint": mountpoint, "log": str(log_path), "rc_addr": rc_addr}
    (state_dir / f"{server['id']}.json").write_text(json.dumps(state, indent=2), encoding="utf-8")
    return state


def unmount_server(server: dict) -> None:
    state_file = rsshmount.app_state_dir() / f"{server['id']}.json"
    if not state_file.exists():
        raise RuntimeError("This server is not recorded as mounted.")
    state = json.loads(state_file.read_text(encoding="utf-8"))
    pid = str(state["pid"])
    if os.name == "nt":
        command = running_rclone_processes().get(int(pid), "")
        if not command:
            state_file.unlink(missing_ok=True)
            return
        if not command_matches_state(command, state):
            state_file.unlink(missing_ok=True)
            raise RuntimeError("Recorded PID no longer belongs to this mount. Removed stale state; the current rclone process was not stopped.")
    elif not pid_is_running(int(pid)):
        state_file.unlink(missing_ok=True)
        return
    result = subprocess.run(["taskkill", "/PID", pid, "/T"], text=True, creationflags=create_no_window())
    if result.returncode != 0:
        run(["taskkill", "/PID", pid, "/T", "/F"])
    state_file.unlink(missing_ok=True)


def refresh_remote_cache(server: dict, rclone: str) -> None:
    state = current_state(server)
    rc_addr = state.get("rc_addr")
    if not rc_addr:
        raise RuntimeError("This mount was created without rclone RC. Remount it before refreshing the directory cache.")
    run([rclone, "--rc-addr", rc_addr, "rc", "vfs/forget"], capture=True)


def startup_command(server_id: str) -> str:
    if getattr(sys, "frozen", False):
        return f'"{sys.executable}" --mount-id "{server_id}"'
    pythonw = shutil.which("pythonw.exe") or shutil.which("python.exe") or "python"
    return f'"{pythonw}" "{Path(__file__).resolve()}" --mount-id "{server_id}"'


def enable_startup(server: dict) -> None:
    task_name = f"SSHMountMate-{server['id']}"
    run(["schtasks", "/Create", "/TN", task_name, "/SC", "ONLOGON", "/TR", startup_command(server["id"]), "/F"])


def disable_startup(server: dict) -> None:
    task_name = f"SSHMountMate-{server['id']}"
    subprocess.run(["schtasks", "/Delete", "/TN", task_name, "/F"], text=True, creationflags=create_no_window())


def headless_mount(server_id: str) -> int:
    servers = load_servers()
    server = next((item for item in servers if item.get("id") == server_id), None)
    if not server:
        return 2
    rclone = resolve_rclone_path()
    if not rclone:
        return 3
    mount_server(server, rclone)
    return 0


class Tooltip:
    def __init__(self, widget, text: str):
        self.widget = widget
        self.text = text
        self.tip = None
        widget.bind("<Enter>", self.show)
        widget.bind("<Leave>", self.hide)

    def show(self, _event=None) -> None:
        if self.tip or not self.text:
            return
        x = self.widget.winfo_rootx() + 18
        y = self.widget.winfo_rooty() + self.widget.winfo_height() + 8
        self.tip = Toplevel(self.widget)
        self.tip.wm_overrideredirect(True)
        self.tip.wm_geometry(f"+{x}+{y}")
        Label(self.tip, text=self.text, bg="#f7f7d0", fg="#222222", padx=6, pady=3, font=("Segoe UI", 9)).pack()

    def hide(self, _event=None) -> None:
        if self.tip:
            self.tip.destroy()
            self.tip = None


class App:
    def __init__(self, root: Tk):
        self.root = root
        self.root.title(APP_TITLE)
        self.root.geometry("760x460")
        self.settings = load_settings()
        self.lang = effective_language(self.settings)
        configure_default_fonts(self.root, self.lang)
        self.servers: list[dict] = []
        self.rclone = ""

        self.status = StringVar(value=self.t("loading_configs"))
        self.dep_status = StringVar(value="")
        self.prompted_deps = False
        self.configs_loaded = False
        self.status_refreshing = False
        self.dependency_checking = False
        self.mount_status_cache: dict[str, str] = {}
        self.capacity_cache: dict[str, dict] = {}

        self.build()
        self.root.protocol("WM_DELETE_WINDOW", self.exit_app)
        self.refresh_list()
        self.root.after_idle(self.start_background_startup)

    def t(self, key: str, **kwargs) -> str:
        return tr_lang(self.lang, key, **kwargs)

    def status_text(self, status: str) -> str:
        return {
            "mounted": self.t("mounted_status"),
            "stopped": self.t("stopped_status"),
            "stale": self.t("stale_status"),
            "checking": self.t("checking_status"),
        }.get(status, status)

    def rebuild(self) -> None:
        for child in self.root.winfo_children():
            child.destroy()
        self.build()
        self.refresh_list()

    def start_background_startup(self) -> None:
        self.reload_configs_async()
        self.check_dependencies_async()

    def build(self) -> None:
        top = Frame(self.root, padx=10, pady=8)
        top.pack(fill=X)
        Label(top, text="ssh-mountmate").pack(side=LEFT)
        Button(top, text=self.t("settings"), command=self.open_settings).pack(side=RIGHT, padx=6)
        Button(top, text=self.t("add_config"), command=self.add_config).pack(side=RIGHT, padx=6)
        Button(top, text=self.t("refresh"), command=self.reload_configs_async).pack(side=RIGHT)

        body = Frame(self.root, padx=10, pady=4)
        body.pack(fill=BOTH, expand=True)

        self.cards_canvas = Canvas(body, bg="#202020", highlightthickness=0)
        self.cards_scrollbar = Scrollbar(body, orient="vertical", command=self.cards_canvas.yview)
        self.cards_frame = Frame(self.cards_canvas, bg="#202020")
        self.cards_window = self.cards_canvas.create_window((0, 0), window=self.cards_frame, anchor="nw")
        self.cards_frame.bind("<Configure>", lambda _event: self.cards_canvas.configure(scrollregion=self.cards_canvas.bbox("all")))
        self.cards_canvas.bind("<Configure>", lambda event: self.cards_canvas.itemconfigure(self.cards_window, width=event.width))
        self.cards_canvas.configure(yscrollcommand=self.cards_scrollbar.set)
        self.cards_canvas.pack(side=LEFT, fill=BOTH, expand=True)
        self.cards_scrollbar.pack(side=RIGHT, fill=Y)
        self.cards_canvas.bind("<MouseWheel>", self.on_cards_mousewheel)
        self.cards_canvas.bind("<Button-4>", self.on_cards_mousewheel)
        self.cards_canvas.bind("<Button-5>", self.on_cards_mousewheel)
        self.cards_frame.bind("<MouseWheel>", self.on_cards_mousewheel)
        self.cards_frame.bind("<Button-4>", self.on_cards_mousewheel)
        self.cards_frame.bind("<Button-5>", self.on_cards_mousewheel)

        bottom = Frame(self.root, padx=10, pady=8)
        bottom.pack(fill=X)
        Label(bottom, textvariable=self.status).pack(side=LEFT)

    def exit_app(self) -> None:
        self.root.destroy()

    def refresh_list(self) -> None:
        for child in self.cards_frame.winfo_children():
            child.destroy()
        if not self.configs_loaded:
            Label(
                self.cards_frame,
                text=self.t("loading_configs"),
                bg="#202020",
                fg="#bdbdbd",
                font=("Segoe UI", 11),
                pady=26,
            ).pack(fill=X)
            return
        if not self.servers:
            Label(
                self.cards_frame,
                text=self.t("no_configs"),
                bg="#202020",
                fg="#bdbdbd",
                font=("Segoe UI", 11),
                pady=26,
            ).pack(fill=X)
            return
        for server in self.servers:
            server_id = server.get("id", "")
            status = self.mount_status_cache.get(server_id, "checking")
            capacity = self.capacity_cache.get(server_id, {})
            self.add_server_card(server, status, capacity)
        self.bind_cards_mousewheel_recursive(self.cards_frame)

    def reload_configs_async(self) -> None:
        self.status.set(self.t("loading_configs"))

        def worker() -> None:
            servers = load_servers()
            self.root.after(0, lambda: self.apply_loaded_configs(servers))

        threading.Thread(target=worker, daemon=True).start()

    def apply_loaded_configs(self, servers: list[dict]) -> None:
        self.servers = servers
        self.configs_loaded = True
        self.status.set(self.t("ready"))
        self.refresh_list()
        self.refresh_mount_status_async()

    def refresh_mount_status_async(self) -> None:
        if self.status_refreshing:
            return
        self.status_refreshing = True
        servers = [dict(server) for server in self.servers]

        def worker() -> None:
            processes = running_rclone_processes() if os.name == "nt" else None
            statuses: dict[str, str] = {}
            capacities: dict[str, dict] = {}
            for server in servers:
                server_id = server.get("id", "")
                if not server_id:
                    continue
                status = mount_status_with_processes(server, processes) if processes is not None else mount_status(server)
                statuses[server_id] = status
                capacities[server_id] = capacity_info(server, status)
            self.root.after(0, lambda: self.apply_mount_statuses(statuses, capacities))

        threading.Thread(target=worker, daemon=True).start()

    def apply_mount_statuses(self, statuses: dict[str, str], capacities: dict[str, dict]) -> None:
        self.status_refreshing = False
        self.mount_status_cache = statuses
        self.capacity_cache = capacities
        self.refresh_list()

    def on_cards_mousewheel(self, event) -> None:
        if getattr(event, "num", None) == 4:
            direction = -1
        elif getattr(event, "num", None) == 5:
            direction = 1
        else:
            delta = getattr(event, "delta", 0)
            direction = -1 if delta > 0 else 1
        self.cards_canvas.yview_scroll(direction, "units")

    def bind_cards_mousewheel_recursive(self, widget) -> None:
        widget.bind("<MouseWheel>", self.on_cards_mousewheel)
        widget.bind("<Button-4>", self.on_cards_mousewheel)
        widget.bind("<Button-5>", self.on_cards_mousewheel)
        for child in widget.winfo_children():
            self.bind_cards_mousewheel_recursive(child)

    def add_server_card(self, server: dict, status: str = "checking", capacity: dict | None = None) -> None:
        mounted = status == "mounted"
        row_bg = "#2a2a2a" if mounted else "#242424"
        muted = "#7d7d7d"
        fg = "#f1f1f1" if mounted else "#bdbdbd"

        row = Frame(self.cards_frame, bg=row_bg, padx=12, pady=10)
        row.pack(fill=X, pady=4)

        left = Frame(row, bg=row_bg, width=90)
        left.pack(side=LEFT, fill="y")
        Label(left, text="🛡", bg=row_bg, fg=fg, font=("Segoe UI Emoji", 28)).pack(anchor="w")
        Label(left, text=self.status_text(status), bg=row_bg, fg=muted, font=(FONT_FAMILY_ZH if self.lang == "zh" else FONT_FAMILY_EN, 9)).pack(anchor="w", pady=(6, 0))

        mid = Frame(row, bg=row_bg)
        mid.pack(side=LEFT, fill=BOTH, expand=True)
        drive = display_mountpoint(server)
        capacity = capacity or {}
        if mounted and capacity:
            capacity_label = self.t(
                "capacity_used",
                used=format_capacity_bytes(int(capacity["used"])),
                total=format_capacity_bytes(int(capacity["total"])),
                percent=int(capacity["percent"]),
            )
        else:
            capacity_label = self.t("checking_capacity") if mounted else self.t("unknown_capacity")
        font_family = FONT_FAMILY_ZH if self.lang == "zh" else FONT_FAMILY_EN
        Label(mid, text=f"{drive}  {server.get('name') or server.get('id')}", bg=row_bg, fg=fg, font=(font_family, 13, "bold")).pack(anchor="w")
        Label(mid, text=capacity_label, bg=row_bg, fg="#c8c8c8", font=(font_family, 10)).pack(anchor="w")
        self.capacity_bar(mid, int(capacity.get("percent", 0)) if mounted and capacity else None, row_bg, muted).pack(fill=X, pady=(5, 4))
        Label(mid, text=f"{server.get('user', '')}@{server.get('host', '')}", bg=row_bg, fg=muted, font=(font_family, 10)).pack(anchor="e", fill=X)
        Label(mid, text=server.get("remote_path") or "~", bg=row_bg, fg=muted, font=(font_family, 10)).pack(anchor="e", fill=X)

        actions = Frame(row, bg=row_bg)
        actions.pack(side=RIGHT)
        self.icon_button(actions, "■" if mounted else "▶", self.t("unmount") if mounted else self.t("mount"), lambda s=server: self.toggle_mount(s)).pack(side=LEFT, padx=4)
        self.icon_button(actions, "↻", self.t("refresh_remote") if mounted and current_state(server).get("rc_addr") else self.t("refresh_unavailable"), lambda s=server: self.refresh_remote(s), enabled=mounted and bool(current_state(server).get("rc_addr"))).pack(side=LEFT, padx=4)
        self.icon_button(actions, "📂", self.t("open_folder"), lambda s=server: self.open_folder(s), enabled=mounted).pack(side=LEFT, padx=4)
        self.icon_button(actions, "✎", self.t("edit_mounted_disabled") if mounted else self.t("edit_mount"), lambda s=server: self.edit_server(s), enabled=not mounted).pack(side=LEFT, padx=4)
        self.icon_button(actions, "☰", self.t("view_log"), lambda s=server: self.open_log(s)).pack(side=LEFT, padx=4)
        self.icon_button(actions, "🗑", self.t("delete_config"), lambda s=server: self.delete_server(s), enabled=not mounted).pack(side=LEFT, padx=4)

    def icon_button(self, parent, text: str, tooltip: str, command, *, enabled: bool = True):
        button = Button(parent, text=text, width=3, height=1, command=command, font=("Segoe UI Emoji", 14))
        if not enabled:
            button.configure(fg="#777777", command=lambda: None)
        Tooltip(button, tooltip)
        return button

    def capacity_bar(self, parent, percent: int | None, bg: str, muted: str) -> Canvas:
        canvas = Canvas(parent, height=8, bg=bg, highlightthickness=0)

        def redraw(event=None) -> None:
            width = max(canvas.winfo_width(), 1)
            height = 8
            canvas.delete("all")
            canvas.create_rectangle(0, 0, width, height, fill="#3a3a3a", outline="")
            if percent is None:
                canvas.create_rectangle(0, 0, width, height, fill="#303030", outline="")
                return
            fill_width = int(width * max(0, min(percent, 100)) / 100)
            color = "#52b788" if percent < 80 else "#f0b429" if percent < 92 else "#e55353"
            canvas.create_rectangle(0, 0, fill_width, height, fill=color, outline="")
            canvas.create_line(0, height - 1, width, height - 1, fill=muted)

        canvas.bind("<Configure>", redraw)
        canvas.after_idle(redraw)
        return canvas

    def check_dependencies_async(self) -> None:
        if self.dependency_checking:
            return
        self.dependency_checking = True
        self.dep_status.set(self.t("checking_deps"))
        threading.Thread(target=self.check_dependencies, daemon=True).start()

    def check_dependencies(self) -> None:
        rclone_path = resolve_rclone_path()
        rclone_ok = bool(rclone_path)
        winfsp_ok = winfsp_installed() if os.name == "nt" else True
        ssh_ok = ssh_installed()
        missing = []
        if not rclone_ok:
            missing.append("rclone")
        if not winfsp_ok:
            missing.append("WinFsp")
        if not ssh_ok:
            missing.append("OpenSSH")
        self.root.after(0, lambda: self.apply_dependency_result(rclone_path, rclone_ok, winfsp_ok, ssh_ok, missing))

    def apply_dependency_result(self, rclone_path: str, rclone_ok: bool, winfsp_ok: bool, ssh_ok: bool, missing: list[str]) -> None:
        self.dependency_checking = False
        self.rclone = rclone_path
        self.dep_status.set(
            self.t(
                "deps_status",
                rclone=self.t("ok") if rclone_ok else self.t("missing"),
                winfsp=self.t("ok") if winfsp_ok else self.t("missing"),
                ssh=self.t("ok") if ssh_ok else self.t("missing"),
            )
        )
        if missing and not self.prompted_deps:
            self.prompted_deps = True
            self.prompt_install_deps(missing)

    def prompt_install_deps(self, missing: list[str]) -> None:
        if messagebox.askyesno(APP_TITLE, self.t("missing_dependencies", items=", ".join(missing))):
            self.install_deps_async()

    def open_settings(self) -> None:
        self.check_dependencies_async()
        settings = load_settings()
        window = Toplevel(self.root)
        window.title(self.t("settings"))
        window.geometry("560x640")
        frame = Frame(window, padx=14, pady=14)
        frame.pack(fill=BOTH, expand=True)
        Label(frame, textvariable=self.dep_status, anchor="w", justify=LEFT).pack(fill=X, pady=(0, 12))
        Button(frame, text=self.t("check_dependencies"), command=self.check_dependencies_async).pack(fill=X, pady=3)
        Button(frame, text=self.t("install_missing_dependencies"), command=self.install_deps_async).pack(fill=X, pady=3)

        ttk.Separator(frame).pack(fill=X, pady=12)

        cache_root = StringVar(value=settings.get("cache_root", default_settings()["cache_root"]))
        cache_mode = StringVar(value=settings.get("vfs_cache_mode", "writes"))
        cache_max_size = StringVar(value=setting_to_choice(settings.get("vfs_cache_max_size", ""), CACHE_SIZE_CHOICES[0]))
        cache_max_age = StringVar(value=setting_to_choice(settings.get("vfs_cache_max_age", ""), CACHE_AGE_CHOICES[0]))
        min_free_space = StringVar(value=setting_to_choice(settings.get("vfs_cache_min_free_space", ""), MIN_FREE_CHOICES[0]))
        write_back = StringVar(value=setting_to_choice(settings.get("vfs_write_back", ""), WRITE_BACK_CHOICES[0]))
        dir_cache_time = StringVar(value=setting_to_choice(settings.get("dir_cache_time", ""), DIR_CACHE_TIME_CHOICES[0]))
        buffer_size = StringVar(value=setting_to_choice(settings.get("buffer_size", ""), BUFFER_SIZE_CHOICES[0]))
        startup_all = BooleanVar(value=bool(settings.get("startup_all", False)))
        language = StringVar(value=language_choice_from_setting(settings.get("language", "auto")))

        lang_row = Frame(frame)
        lang_row.pack(fill=X, pady=3)
        Label(lang_row, text=self.t("language"), width=16, anchor="w").pack(side=LEFT)
        ttk.Combobox(lang_row, values=list(LANGUAGE_CHOICES.values()), textvariable=language, state="readonly").pack(side=LEFT, fill=X, expand=True)

        cache_row = Frame(frame)
        cache_row.pack(fill=X, pady=3)
        Label(cache_row, text=self.t("cache_root"), width=16, anchor="w").pack(side=LEFT)
        Entry(cache_row, textvariable=cache_root).pack(side=LEFT, fill=X, expand=True)
        Button(cache_row, text="...", command=lambda: self.pick_cache_root(cache_root)).pack(side=RIGHT)

        mode_row = Frame(frame)
        mode_row.pack(fill=X, pady=3)
        Label(mode_row, text=self.t("vfs_cache_mode"), width=16, anchor="w").pack(side=LEFT)
        ttk.Combobox(mode_row, values=["off", "minimal", "writes", "full"], textvariable=cache_mode, state="readonly").pack(side=LEFT, fill=X, expand=True)

        size_row = Frame(frame)
        size_row.pack(fill=X, pady=3)
        Label(size_row, text=self.t("max_cache_size"), width=16, anchor="w").pack(side=LEFT)
        ttk.Combobox(size_row, values=CACHE_SIZE_CHOICES, textvariable=cache_max_size, state="readonly").pack(side=LEFT, fill=X, expand=True)

        age_row = Frame(frame)
        age_row.pack(fill=X, pady=3)
        Label(age_row, text=self.t("max_cache_age"), width=16, anchor="w").pack(side=LEFT)
        ttk.Combobox(age_row, values=CACHE_AGE_CHOICES, textvariable=cache_max_age, state="readonly").pack(side=LEFT, fill=X, expand=True)

        min_free_row = Frame(frame)
        min_free_row.pack(fill=X, pady=3)
        Label(min_free_row, text=self.t("min_free_space"), width=16, anchor="w").pack(side=LEFT)
        ttk.Combobox(min_free_row, values=MIN_FREE_CHOICES, textvariable=min_free_space, state="readonly").pack(side=LEFT, fill=X, expand=True)

        write_back_row = Frame(frame)
        write_back_row.pack(fill=X, pady=3)
        Label(write_back_row, text=self.t("write_back"), width=16, anchor="w").pack(side=LEFT)
        ttk.Combobox(write_back_row, values=WRITE_BACK_CHOICES, textvariable=write_back, state="readonly").pack(side=LEFT, fill=X, expand=True)

        dir_cache_row = Frame(frame)
        dir_cache_row.pack(fill=X, pady=3)
        Label(dir_cache_row, text=self.t("dir_cache_time"), width=16, anchor="w").pack(side=LEFT)
        ttk.Combobox(dir_cache_row, values=DIR_CACHE_TIME_CHOICES, textvariable=dir_cache_time, state="readonly").pack(side=LEFT, fill=X, expand=True)

        buffer_row = Frame(frame)
        buffer_row.pack(fill=X, pady=3)
        Label(buffer_row, text=self.t("buffer_size"), width=16, anchor="w").pack(side=LEFT)
        ttk.Combobox(buffer_row, values=BUFFER_SIZE_CHOICES, textvariable=buffer_size, state="readonly").pack(side=LEFT, fill=X, expand=True)

        Checkbutton(frame, text=self.t("startup_all"), variable=startup_all).pack(anchor="w", pady=8)

        def save() -> None:
            new_settings = load_settings()
            new_settings.update(
                {
                    "cache_root": cache_root.get().strip() or default_settings()["cache_root"],
                    "vfs_cache_mode": cache_mode.get() or "writes",
                    "vfs_cache_max_size": choice_to_setting(cache_max_size.get().strip()),
                    "vfs_cache_max_age": choice_to_setting(cache_max_age.get().strip()),
                    "vfs_cache_min_free_space": choice_to_setting(min_free_space.get().strip()),
                    "vfs_write_back": choice_to_setting(write_back.get().strip()),
                    "dir_cache_time": choice_to_setting(dir_cache_time.get().strip()),
                    "buffer_size": choice_to_setting(buffer_size.get().strip()),
                    "startup_all": bool(startup_all.get()),
                    "language": language_setting_from_choice(language.get()),
                }
            )
            save_settings(new_settings)
            old_lang = self.lang
            self.settings = new_settings
            self.lang = effective_language(new_settings)
            if self.lang != old_lang:
                configure_default_fonts(self.root, self.lang)
                self.rebuild()
            self.apply_startup_setting(new_settings["startup_all"])
            self.status.set(self.t("settings_saved"))
            window.destroy()

        Button(frame, text=self.t("save_settings"), command=save).pack(fill=X, pady=(12, 0))

    def pick_cache_root(self, variable: StringVar) -> None:
        path = filedialog.askdirectory(initialdir=variable.get() or str(Path.home()))
        if path:
            variable.set(path)

    def apply_startup_setting(self, enabled: bool) -> None:
        if os.name != "nt":
            return
        for server in self.servers:
            try:
                if enabled:
                    enable_startup(server)
                else:
                    disable_startup(server)
            except Exception:
                pass

    def install_deps_async(self) -> None:
        threading.Thread(target=self.install_deps, daemon=True).start()

    def install_deps(self) -> None:
        try:
            self.root.after(0, lambda: self.status.set(self.t("installing_deps")))
            if not resolve_rclone_path():
                install_rclone()
            if os.name == "nt" and not winfsp_installed():
                install_winfsp()
            if os.name == "nt" and not ssh_installed():
                install_openssh_client()
            rclone_path = resolve_rclone_path()
            self.root.after(0, lambda: self.on_dependency_install_done(rclone_path))
        except Exception as exc:
            message = str(exc)
            self.root.after(0, lambda: self.on_dependency_install_failed(message))

    def on_dependency_install_done(self, rclone_path: str) -> None:
        self.rclone = rclone_path
        self.status.set(self.t("deps_complete"))
        self.check_dependencies_async()

    def on_dependency_install_failed(self, message: str) -> None:
        self.status.set(self.t("deps_failed"))
        self.show_error(message)

    def show_text_window(self, title: str, content: str) -> None:
        window = Toplevel(self.root)
        window.title(title)
        window.geometry("720x420")
        frame = Frame(window, padx=10, pady=10)
        frame.pack(fill=BOTH, expand=True)
        scrollbar = Scrollbar(frame)
        text = Text(frame, wrap="word", yscrollcommand=scrollbar.set)
        scrollbar.configure(command=text.yview)
        text.insert("1.0", content)
        text.configure(state="disabled")
        text.pack(side=LEFT, fill=BOTH, expand=True)
        scrollbar.pack(side=RIGHT, fill=Y)
        buttons = Frame(window, padx=10, pady=8)
        buttons.pack(fill=X)

        def copy_content() -> None:
            self.root.clipboard_clear()
            self.root.clipboard_append(content)
            self.status.set(self.t("copied"))

        Button(buttons, text=self.t("copy"), command=copy_content).pack(side=RIGHT)
        Button(buttons, text=self.t("close"), command=window.destroy).pack(side=RIGHT, padx=6)

    def show_error(self, message: str) -> None:
        self.show_text_window(self.t("error_details"), message)

    def open_log(self, server: dict) -> None:
        path = current_log_path(server)
        try:
            lines = path.read_text(encoding="utf-8", errors="ignore").splitlines()
            content = f"{path}\n\n" + "\n".join(lines[-300:])
        except OSError as exc:
            content = f"{path}\n\n{exc}"
        self.show_text_window(self.t("mount_log"), content)

    def refresh_remote(self, server: dict) -> None:
        try:
            refresh_remote_cache(server, self.current_rclone())
            self.status.set(self.t("remote_refreshed"))
        except Exception as exc:
            self.show_error(str(exc))

    def add_config(self) -> None:
        dialog = ServerDialog(self.root, rclone=self.current_rclone(), lang=self.lang)
        self.root.wait_window(dialog.window)
        if dialog.result:
            self.servers.append(dialog.result)
            save_servers(self.servers)
            if load_settings().get("startup_all"):
                try:
                    enable_startup(dialog.result)
                except Exception:
                    pass
            self.refresh_list()
            self.refresh_mount_status_async()

    def edit_server(self, server: dict) -> None:
        try:
            index = self.servers.index(server)
        except ValueError:
            return
        dialog = ServerDialog(self.root, rclone=self.current_rclone(), existing=server, lang=self.lang)
        self.root.wait_window(dialog.window)
        if dialog.result:
            self.servers[index] = dialog.result
            save_servers(self.servers)
            self.refresh_list()
            self.refresh_mount_status_async()

    def toggle_mount(self, server: dict) -> None:
        if verified_mount_status(server) == "mounted":
            try:
                unmount_server(server)
                self.status.set(self.t("unmounted"))
            except Exception as exc:
                self.show_error(str(exc))
        else:
            try:
                state = mount_server(server, self.current_rclone())
                self.status.set(self.t("mounted_at", remote=state["remote"], mountpoint=state["mountpoint"]))
            except Exception as exc:
                self.show_error(str(exc))
        self.refresh_list()
        self.refresh_mount_status_async()

    def open_folder(self, server: dict) -> None:
        if verified_mount_status(server) != "mounted":
            messagebox.showinfo(APP_TITLE, self.t("mount_before_open"))
            return
        mountpoint = current_mountpoint(server)
        try:
            if os.name == "nt":
                os.startfile(mountpoint)
            elif sys.platform == "darwin":
                subprocess.Popen(["open", mountpoint])
            else:
                subprocess.Popen(["xdg-open", mountpoint])
        except Exception as exc:
            self.show_error(str(exc))

    def delete_server(self, server: dict) -> None:
        status = verified_mount_status(server)
        name = server.get("name") or server.get("id")
        if status == "mounted":
            if not messagebox.askyesno(APP_TITLE, self.t("delete_mounted_confirm", name=name)):
                return
            try:
                unmount_server(server)
            except Exception as exc:
                self.show_error(str(exc))
                return
        else:
            if not messagebox.askyesno(APP_TITLE, self.t("delete_confirm", name=name)):
                return
        state_file = server_state_file(server)
        if state_file.exists() and verified_mount_status(server) != "mounted":
            state_file.unlink(missing_ok=True)
        self.servers = [item for item in self.servers if item is not server and item.get("id") != server.get("id")]
        try:
            disable_startup(server)
        except Exception:
            pass
        save_servers(self.servers)
        self.status.set(self.t("deleted", name=name))
        self.refresh_list()
        self.refresh_mount_status_async()

    def current_rclone(self) -> str:
        if not self.rclone:
            self.rclone = resolve_rclone_path()
        return self.rclone


class ServerDialog:
    def __init__(self, root: Tk, *, rclone: str, lang: str, existing: dict | None = None):
        self.result = None
        self.rclone = rclone
        self.lang = lang
        self.existing = existing or {}
        existing_source = self.existing.get("source")
        if not existing_source and self.existing.get("mode") == "ssh_config":
            existing_source = "ssh_config"
        self.source = StringVar(value=existing_source or "ssh_config")
        self.auth = StringVar(value=self.existing.get("auth", "key"))
        self.values: dict[str, Entry] = {}
        self.window = Toplevel(root)
        self.window.title(self.t("edit_config_title") if existing else self.t("add_config_title"))
        self.window.geometry("500x520")
        self.window.minsize(460, 360)
        self.window.resizable(True, True)
        self.canvas = Canvas(self.window, highlightthickness=0)
        self.scrollbar = Scrollbar(self.window, orient="vertical", command=self.canvas.yview)
        self.form = Frame(self.canvas)
        self.form.bind("<Configure>", lambda _event: self.canvas.configure(scrollregion=self.canvas.bbox("all")))
        self.canvas.create_window((0, 0), window=self.form, anchor="nw")
        self.canvas.configure(yscrollcommand=self.scrollbar.set)
        self.canvas.pack(side=LEFT, fill=BOTH, expand=True)
        self.scrollbar.pack(side=RIGHT, fill=Y)
        self.window.bind("<MouseWheel>", self.on_mousewheel)
        self.window.bind("<Button-4>", self.on_mousewheel)
        self.window.bind("<Button-5>", self.on_mousewheel)
        self.canvas.bind("<MouseWheel>", self.on_mousewheel)
        self.canvas.bind("<Button-4>", self.on_mousewheel)
        self.canvas.bind("<Button-5>", self.on_mousewheel)
        self.form.bind("<MouseWheel>", self.on_mousewheel)
        self.form.bind("<Button-4>", self.on_mousewheel)
        self.form.bind("<Button-5>", self.on_mousewheel)
        self.build()
        self.window.bind_all("<MouseWheel>", self.on_mousewheel)
        self.window.bind_all("<Button-4>", self.on_mousewheel)
        self.window.bind_all("<Button-5>", self.on_mousewheel)
        self.window.bind("<Destroy>", self.on_destroy)

    def t(self, key: str, **kwargs) -> str:
        return tr_lang(self.lang, key, **kwargs)

    def row(self, label: str, key: str, default: str = "", browse=False, secret=False):
        frame = Frame(self.form, padx=10, pady=4)
        frame.pack(fill=X)
        Label(frame, text=label, width=14, anchor="w").pack(side=LEFT)
        entry = Entry(frame, show="*" if secret else None)
        entry.insert(0, default)
        entry.pack(side=LEFT, fill=X, expand=True)
        self.values[key] = entry
        if browse:
            Button(frame, text="...", command=lambda: self.pick_file(key)).pack(side=RIGHT)
        return entry

    def row_combo(self, label: str, key: str, values: list[str], default: str = ""):
        frame = Frame(self.form, padx=10, pady=4)
        frame.pack(fill=X)
        Label(frame, text=label, width=14, anchor="w").pack(side=LEFT)
        combo = ttk.Combobox(frame, values=values)
        if default:
            combo.set(default)
        elif values:
            combo.set(values[0])
        combo.pack(side=LEFT, fill=X, expand=True)
        self.values[key] = combo
        return combo

    def row_remote_path(self, remote_path: str) -> None:
        base, suffix = split_remote_path(remote_path)
        frame = Frame(self.form, padx=10, pady=4)
        frame.pack(fill=X)
        Label(frame, text=self.t("remote_path"), width=14, anchor="w").pack(side=LEFT)
        combo = ttk.Combobox(frame, values=["$HOME", "/"], width=8, state="readonly")
        combo.set(base)
        combo.pack(side=LEFT)
        self.values["remote_base"] = combo
        entry = Entry(frame)
        entry.insert(0, suffix)
        entry.pack(side=LEFT, fill=X, expand=True, padx=(6, 0))
        self.values["remote_suffix"] = entry

    def build(self) -> None:
        source_frame = Frame(self.form, padx=10, pady=4)
        source_frame.pack(fill=X)
        Label(source_frame, text=self.t("source"), width=14, anchor="w").pack(side=LEFT)
        ttk.Radiobutton(source_frame, text=self.t("ssh_config"), variable=self.source, value="ssh_config", command=self.on_source_changed).pack(side=LEFT)
        ttk.Radiobutton(source_frame, text=self.t("manual"), variable=self.source, value="manual", command=self.on_source_changed).pack(side=LEFT)

        hosts = list_ssh_config_hosts()
        host_default = self.existing.get("host_alias") or (hosts[0] if hosts else "")
        self.host_combo = self.row_combo(self.t("ssh_host"), "host_alias", hosts, host_default)
        self.host_combo.bind("<<ComboboxSelected>>", self.on_ssh_host_selected)

        self.row(self.t("name"), "name", self.existing.get("name", ""))
        self.row(self.t("ip_host"), "host", self.existing.get("host", ""))
        self.row(self.t("user"), "user", self.existing.get("user", ""))
        self.row(self.t("port"), "port", str(self.existing.get("port") or "22"))

        auth_frame = Frame(self.form, padx=10, pady=4)
        auth_frame.pack(fill=X)
        Label(auth_frame, text=self.t("auth"), width=14, anchor="w").pack(side=LEFT)
        ttk.Radiobutton(auth_frame, text=self.t("key"), variable=self.auth, value="key").pack(side=LEFT)
        ttk.Radiobutton(auth_frame, text=self.t("password_auth"), variable=self.auth, value="password").pack(side=LEFT)
        self.row(self.t("key_file"), "key_file", self.existing.get("key_file", ""), browse=True)
        self.row(self.t("key_passphrase"), "key_passphrase", secret=True)
        self.row(self.t("password"), "password", secret=True)

        self.row_remote_path(self.existing.get("remote_path", ""))
        self.row_combo(self.t("mountpoint"), "mountpoint", mountpoint_choices(), self.existing.get("mountpoint") or "Auto")

        buttons = Frame(self.form, padx=10, pady=10)
        buttons.pack(fill=X)
        Button(buttons, text=self.t("save"), command=self.save).pack(side=RIGHT)
        Button(buttons, text=self.t("cancel"), command=self.window.destroy).pack(side=RIGHT, padx=6)

        self.update_source_controls()
        if self.source.get() == "ssh_config" and not self.existing and host_default:
            self.apply_ssh_defaults(host_default)
        self.bind_mousewheel_recursive(self.form)

    def on_mousewheel(self, event) -> None:
        if getattr(event, "num", None) == 4:
            direction = -1
        elif getattr(event, "num", None) == 5:
            direction = 1
        else:
            delta = getattr(event, "delta", 0)
            direction = -1 if delta > 0 else 1
        self.canvas.yview_scroll(direction, "units")

    def bind_mousewheel_recursive(self, widget) -> None:
        widget.bind("<MouseWheel>", self.on_mousewheel)
        widget.bind("<Button-4>", self.on_mousewheel)
        widget.bind("<Button-5>", self.on_mousewheel)
        for child in widget.winfo_children():
            self.bind_mousewheel_recursive(child)

    def on_destroy(self, event) -> None:
        if event.widget is self.window:
            self.window.unbind_all("<MouseWheel>")
            self.window.unbind_all("<Button-4>")
            self.window.unbind_all("<Button-5>")

    def pick_file(self, key: str) -> None:
        path = filedialog.askopenfilename()
        if path:
            self.set_value(key, path)

    def get(self, key: str) -> str:
        entry = self.values.get(key)
        return entry.get().strip() if entry else ""

    def set_value(self, key: str, value: str) -> None:
        entry = self.values.get(key)
        if not entry:
            return
        entry.delete(0, END)
        entry.insert(0, value or "")

    def update_source_controls(self) -> None:
        state = "readonly" if self.source.get() == "ssh_config" else "disabled"
        self.host_combo.configure(state=state)

    def on_source_changed(self) -> None:
        self.update_source_controls()
        if self.source.get() == "ssh_config" and self.get("host_alias"):
            self.apply_ssh_defaults(self.get("host_alias"))

    def on_ssh_host_selected(self, _event=None) -> None:
        self.apply_ssh_defaults(self.get("host_alias"))

    def apply_ssh_defaults(self, host_alias: str) -> None:
        try:
            defaults = ssh_config_defaults(host_alias)
        except Exception as exc:
            messagebox.showerror(APP_TITLE, str(exc))
            return
        for key in ["name", "host", "user", "port", "key_file"]:
            self.set_value(key, defaults.get(key, ""))
        self.auth.set("key" if defaults.get("key_file") else self.auth.get())

    def save(self) -> None:
        source = self.source.get()
        name = self.get("name") or self.get("host_alias") or self.get("host")
        if not name:
            messagebox.showerror(APP_TITLE, self.t("name_required"))
            return
        host = self.get("host")
        user = self.get("user")
        if not host or not user:
            messagebox.showerror(APP_TITLE, self.t("host_user_required"))
            return

        server_id = self.existing.get("id") or "".join(ch if ch.isalnum() or ch in "._-" else "_" for ch in name)
        mountpoint = self.get("mountpoint")
        if mountpoint.lower() == "auto":
            mountpoint = ""

        result = {
            "id": server_id,
            "name": name,
            "mode": "manual",
            "source": source,
            "host_alias": self.get("host_alias") if source == "ssh_config" else "",
            "host": host,
            "user": user,
            "port": self.get("port") or "22",
            "auth": self.auth.get(),
            "key_file": self.get("key_file"),
            "remote_path": compose_remote_path(self.get("remote_base"), self.get("remote_suffix")),
            "mountpoint": mountpoint,
            "cache_mode": self.existing.get("cache_mode", ""),
        }

        if self.auth.get() == "password":
            password = self.get("password")
            if not password and self.existing.get("password_obscured"):
                result["password_obscured"] = self.existing["password_obscured"]
            elif not password:
                messagebox.showerror(APP_TITLE, self.t("password_required"))
                return
            else:
                try:
                    result["password_obscured"] = obscure_password(self.rclone, password)
                except Exception as exc:
                    messagebox.showerror(APP_TITLE, str(exc))
                    return
        elif self.auth.get() == "key":
            key_passphrase = self.get("key_passphrase")
            if key_passphrase:
                try:
                    result["key_pass_obscured"] = obscure_password(self.rclone, key_passphrase)
                except Exception as exc:
                    messagebox.showerror(APP_TITLE, str(exc))
                    return
            elif self.existing.get("key_pass_obscured"):
                result["key_pass_obscured"] = self.existing["key_pass_obscured"]
        self.result = result
        self.window.destroy()


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--mount-id")
    args = parser.parse_args()
    if args.mount_id:
        return headless_mount(args.mount_id)
    root = Tk()
    App(root)
    root.mainloop()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
