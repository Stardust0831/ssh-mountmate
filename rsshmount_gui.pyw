#!/usr/bin/env python3
import argparse
import glob
import json
import os
import shutil
import subprocess
import sys
import threading
import time
from pathlib import Path
from tkinter import BOTH, END, LEFT, RIGHT, X, Y, BooleanVar, Button, Canvas, Checkbutton, Entry, Frame, Label, Listbox, Scrollbar, StringVar, Tk, Toplevel, filedialog, messagebox
from tkinter import ttk

import rsshmount


APP_TITLE = "SSH MountMate"
CACHE_SIZE_CHOICES = ["default (off)", "1G", "5G", "10G", "20G", "50G", "100G", "500G"]
CACHE_AGE_CHOICES = ["default (1h0m0s)", "5m", "15m", "30m", "1h", "6h", "24h", "168h"]
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
        "startup_all": False,
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


def bundled_dir() -> Path:
    if getattr(sys, "frozen", False):
        return Path(sys.executable).resolve().parent
    return Path(__file__).resolve().parent


def bundled_rclone_path() -> Path:
    return bundled_dir() / "bin" / ("rclone.exe" if os.name == "nt" else "rclone")


def resolve_rclone_path() -> str:
    bundled = bundled_rclone_path()
    if bundled.exists():
        return str(bundled)
    found = shutil.which("rclone.exe" if os.name == "nt" else "rclone") or shutil.which("rclone")
    return found or ""


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


def display_mountpoint(server: dict) -> str:
    mountpoint = current_mountpoint(server)
    return mountpoint if mountpoint else "Auto"


def capacity_text(server: dict) -> str:
    if mount_status(server) != "mounted":
        return "unknown capacity"
    mountpoint = current_mountpoint(server)
    try:
        usage = shutil.disk_usage(mountpoint)
    except OSError:
        return "unknown capacity"
    total = usage.total / (1024 ** 3)
    free = usage.free / (1024 ** 3)
    return f"{free:.1f} GiB free / {total:.1f} GiB"


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


def run_visible_winget_install(title: str, package_id: str) -> None:
    app_dir().mkdir(parents=True, exist_ok=True)
    script = app_dir() / f"install-{package_id.replace('.', '-')}.cmd"
    script.write_text(
        "\n".join(
            [
                "@echo off",
                f"echo Installing {package_id} with winget...",
                f'winget install --id "{package_id}" -e --accept-package-agreements --accept-source-agreements',
                "set RC=%ERRORLEVEL%",
                'if "%RC%"=="0" (',
                "  echo.",
                "  echo Installation succeeded. This window will close in 5 seconds...",
                "  timeout /t 5 /nobreak",
                "  exit /b 0",
                ")",
                "echo.",
                "echo Installation failed with code %RC%.",
                "echo This window will stay open so you can review the output.",
                "cmd /k",
                "exit /b %RC%",
            ]
        ),
        encoding="utf-8",
    )
    subprocess.run(
        ["cmd.exe", "/c", f'call "{script}"'],
        creationflags=getattr(subprocess, "CREATE_NEW_CONSOLE", 0),
        check=True,
    )


def install_rclone() -> None:
    if resolve_rclone_path():
        return
    if os.name != "nt" or not shutil.which("winget"):
        raise RuntimeError("rclone is missing and winget is not available.")
    run_visible_winget_install("rclone", "Rclone.Rclone")


def install_winfsp() -> None:
    if shutil.which("winget"):
        run_visible_winget_install("WinFsp", "WinFsp.WinFsp")
        return
    raise RuntimeError("WinFsp is missing and winget is not available.")


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
    if mount_status(server) == "mounted":
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

    cmd = [
        rclone,
        "--config",
        str(rsshmount.rclone_config_path()),
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
        "--dir-cache-time",
        "30s",
        "--volname",
        server.get("name") or remote_name(server),
    ]
    if settings.get("vfs_cache_max_size"):
        cmd.extend(["--vfs-cache-max-size", settings["vfs_cache_max_size"]])
    if settings.get("vfs_cache_max_age"):
        cmd.extend(["--vfs-cache-max-age", settings["vfs_cache_max_age"]])
    if server.get("network_mode"):
        cmd.append("--network-mode")

    log = log_path.open("ab")
    flags = (
        getattr(subprocess, "DETACHED_PROCESS", 0)
        | getattr(subprocess, "CREATE_NEW_PROCESS_GROUP", 0)
        | getattr(subprocess, "CREATE_NO_WINDOW", 0)
    )
    proc = subprocess.Popen(cmd, stdout=log, stderr=subprocess.STDOUT, creationflags=flags)
    time.sleep(2)
    if proc.poll() is not None:
        tail = ""
        try:
            tail = "\n".join(log_path.read_text(encoding="utf-8", errors="ignore").splitlines()[-8:])
        except OSError:
            pass
        raise RuntimeError(f"Mount failed. See log: {log_path}\n{tail}")
    state = {"pid": proc.pid, "server_id": server["id"], "remote": remote, "mountpoint": mountpoint, "log": str(log_path)}
    (state_dir / f"{server['id']}.json").write_text(json.dumps(state, indent=2), encoding="utf-8")
    return state


def unmount_server(server: dict) -> None:
    state_file = rsshmount.app_state_dir() / f"{server['id']}.json"
    if not state_file.exists():
        raise RuntimeError("This server is not recorded as mounted.")
    state = json.loads(state_file.read_text(encoding="utf-8"))
    pid = str(state["pid"])
    if not pid_is_running(int(pid)):
        state_file.unlink(missing_ok=True)
        return
    result = subprocess.run(["taskkill", "/PID", pid, "/T"], text=True, creationflags=create_no_window())
    if result.returncode != 0:
        run(["taskkill", "/PID", pid, "/T", "/F"])
    state_file.unlink(missing_ok=True)


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
        self.servers = load_servers()
        self.rclone = resolve_rclone_path()

        self.status = StringVar(value="Ready")
        self.dep_status = StringVar(value="")
        self.tray_icon = None
        self.prompted_deps = False

        self.build()
        self.root.protocol("WM_DELETE_WINDOW", self.exit_app)
        self.refresh_list()
        self.root.after(700, self.check_dependencies_async)

    def build(self) -> None:
        top = Frame(self.root, padx=10, pady=8)
        top.pack(fill=X)
        Label(top, text="ssh-mountmate").pack(side=LEFT)
        Button(top, text="Settings", command=self.open_settings).pack(side=RIGHT, padx=6)
        Button(top, text="Add config", command=self.add_config).pack(side=RIGHT, padx=6)
        Button(top, text="Refresh", command=self.refresh_list).pack(side=RIGHT)

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

    def setup_tray(self) -> None:
        try:
            import pystray
            from PIL import Image, ImageDraw
        except Exception:
            self.tray_icon = None
            return

        image = Image.new("RGB", (64, 64), "#2457d6")
        draw = ImageDraw.Draw(image)
        draw.rectangle((14, 14, 50, 50), outline="white", width=4)
        draw.line((20, 32, 44, 32), fill="white", width=4)
        menu = pystray.Menu(
            pystray.MenuItem("Show", lambda _icon, _item: self.root.after(0, self.show_window)),
            pystray.MenuItem("Exit", lambda _icon, _item: self.root.after(0, self.exit_app)),
        )
        self.tray_icon = pystray.Icon("SSHMountMate", image, "SSH MountMate", menu)
        self.tray_icon.run_detached()

    def hide_to_tray(self) -> None:
        if self.tray_icon:
            self.root.withdraw()
        else:
            self.root.iconify()

    def show_window(self) -> None:
        self.root.deiconify()
        self.root.lift()
        self.root.focus_force()

    def exit_app(self) -> None:
        if self.tray_icon:
            self.tray_icon.stop()
            self.tray_icon = None
        self.root.destroy()

    def selected(self) -> dict | None:
        selection = self.listbox.curselection()
        if not selection:
            messagebox.showinfo(APP_TITLE, "Select a server first.")
            return None
        return self.servers[selection[0]]

    def refresh_list(self) -> None:
        for child in self.cards_frame.winfo_children():
            child.destroy()
        pid_set = running_pid_set() if os.name == "nt" else None
        for server in self.servers:
            self.add_server_card(server, pid_set)
        self.bind_cards_mousewheel_recursive(self.cards_frame)

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

    def add_server_card(self, server: dict, pid_set: set[int] | None = None) -> None:
        status = mount_status_with_pids(server, pid_set) if pid_set is not None else mount_status(server)
        mounted = status == "mounted"
        row_bg = "#2a2a2a" if mounted else "#242424"
        muted = "#7d7d7d"
        fg = "#f1f1f1" if mounted else "#bdbdbd"

        row = Frame(self.cards_frame, bg=row_bg, padx=12, pady=10)
        row.pack(fill=X, pady=4)

        left = Frame(row, bg=row_bg, width=90)
        left.pack(side=LEFT, fill="y")
        Label(left, text="🛡", bg=row_bg, fg=fg, font=("Segoe UI Emoji", 28)).pack(anchor="w")
        Label(left, text=status, bg=row_bg, fg=muted, font=("Segoe UI", 9)).pack(anchor="w", pady=(6, 0))

        mid = Frame(row, bg=row_bg)
        mid.pack(side=LEFT, fill=BOTH, expand=True)
        drive = display_mountpoint(server)
        Label(mid, text=f"{drive}  {server.get('name') or server.get('id')}", bg=row_bg, fg=fg, font=("Segoe UI", 13, "bold")).pack(anchor="w")
        Label(mid, text=capacity_text(server), bg=row_bg, fg="#c8c8c8", font=("Segoe UI", 10)).pack(anchor="w")
        Label(mid, text=f"{server.get('user', '')}@{server.get('host', '')}", bg=row_bg, fg=muted, font=("Segoe UI", 10)).pack(anchor="e", fill=X)
        Label(mid, text=server.get("remote_path") or "~", bg=row_bg, fg=muted, font=("Segoe UI", 10)).pack(anchor="e", fill=X)

        actions = Frame(row, bg=row_bg)
        actions.pack(side=RIGHT)
        self.icon_button(actions, "■" if mounted else "▶", "Unmount" if mounted else "Mount", lambda s=server: self.toggle_mount(s)).pack(side=LEFT, padx=4)
        self.icon_button(actions, "📂", "Open mounted folder", lambda s=server: self.open_folder(s), enabled=mounted).pack(side=LEFT, padx=4)
        self.icon_button(actions, "✎", "Edit mount information", lambda s=server: self.edit_server(s)).pack(side=LEFT, padx=4)
        self.icon_button(actions, "🗑", "Delete this config", lambda s=server: self.delete_server(s), enabled=not mounted).pack(side=LEFT, padx=4)

    def icon_button(self, parent, text: str, tooltip: str, command, *, enabled: bool = True):
        button = Button(parent, text=text, width=3, height=1, command=command, font=("Segoe UI Emoji", 14))
        if not enabled:
            button.configure(fg="#777777", command=lambda: None)
        Tooltip(button, tooltip)
        return button

    def check_dependencies_async(self) -> None:
        threading.Thread(target=self.check_dependencies, daemon=True).start()

    def missing_dependencies(self) -> list[str]:
        missing = []
        if not resolve_rclone_path():
            missing.append("rclone")
        if os.name == "nt" and not winfsp_installed():
            missing.append("WinFsp")
        if not ssh_installed():
            missing.append("OpenSSH")
        return missing

    def check_dependencies(self) -> None:
        rclone_ok = bool(resolve_rclone_path())
        winfsp_ok = winfsp_installed() if os.name == "nt" else True
        ssh_ok = ssh_installed()
        self.dep_status.set(f"rclone: {'ok' if rclone_ok else 'missing'}    WinFsp: {'ok' if winfsp_ok else 'missing'}    ssh: {'ok' if ssh_ok else 'missing'}")
        missing = self.missing_dependencies()
        if missing and not self.prompted_deps:
            self.prompted_deps = True
            self.root.after(0, lambda: self.prompt_install_deps(missing))

    def prompt_install_deps(self, missing: list[str]) -> None:
        if messagebox.askyesno(APP_TITLE, "Missing dependencies: " + ", ".join(missing) + ". Install now?"):
            self.install_deps_async()

    def open_settings(self) -> None:
        self.check_dependencies()
        settings = load_settings()
        window = Toplevel(self.root)
        window.title("Settings")
        window.geometry("520x430")
        frame = Frame(window, padx=14, pady=14)
        frame.pack(fill=BOTH, expand=True)
        Label(frame, textvariable=self.dep_status, anchor="w", justify=LEFT).pack(fill=X, pady=(0, 12))
        Button(frame, text="Check dependencies", command=self.check_dependencies_async).pack(fill=X, pady=3)
        Button(frame, text="Install missing dependencies", command=self.install_deps_async).pack(fill=X, pady=3)

        ttk.Separator(frame).pack(fill=X, pady=12)

        cache_root = StringVar(value=settings.get("cache_root", default_settings()["cache_root"]))
        cache_mode = StringVar(value=settings.get("vfs_cache_mode", "writes"))
        cache_max_size = StringVar(value=setting_to_choice(settings.get("vfs_cache_max_size", ""), CACHE_SIZE_CHOICES[0]))
        cache_max_age = StringVar(value=setting_to_choice(settings.get("vfs_cache_max_age", ""), CACHE_AGE_CHOICES[0]))
        startup_all = BooleanVar(value=bool(settings.get("startup_all", False)))

        cache_row = Frame(frame)
        cache_row.pack(fill=X, pady=3)
        Label(cache_row, text="Cache root", width=16, anchor="w").pack(side=LEFT)
        Entry(cache_row, textvariable=cache_root).pack(side=LEFT, fill=X, expand=True)
        Button(cache_row, text="...", command=lambda: self.pick_cache_root(cache_root)).pack(side=RIGHT)

        mode_row = Frame(frame)
        mode_row.pack(fill=X, pady=3)
        Label(mode_row, text="VFS cache mode", width=16, anchor="w").pack(side=LEFT)
        ttk.Combobox(mode_row, values=["off", "minimal", "writes", "full"], textvariable=cache_mode, state="readonly").pack(side=LEFT, fill=X, expand=True)

        size_row = Frame(frame)
        size_row.pack(fill=X, pady=3)
        Label(size_row, text="Max cache size", width=16, anchor="w").pack(side=LEFT)
        ttk.Combobox(size_row, values=CACHE_SIZE_CHOICES, textvariable=cache_max_size, state="readonly").pack(side=LEFT, fill=X, expand=True)

        age_row = Frame(frame)
        age_row.pack(fill=X, pady=3)
        Label(age_row, text="Max cache age", width=16, anchor="w").pack(side=LEFT)
        ttk.Combobox(age_row, values=CACHE_AGE_CHOICES, textvariable=cache_max_age, state="readonly").pack(side=LEFT, fill=X, expand=True)

        Checkbutton(frame, text="Mount all configs on Windows login", variable=startup_all).pack(anchor="w", pady=8)

        def save() -> None:
            new_settings = load_settings()
            new_settings.update(
                {
                    "cache_root": cache_root.get().strip() or default_settings()["cache_root"],
                    "vfs_cache_mode": cache_mode.get() or "writes",
                    "vfs_cache_max_size": choice_to_setting(cache_max_size.get().strip()),
                    "vfs_cache_max_age": choice_to_setting(cache_max_age.get().strip()),
                    "startup_all": bool(startup_all.get()),
                }
            )
            save_settings(new_settings)
            self.apply_startup_setting(new_settings["startup_all"])
            self.status.set("Settings saved.")
            window.destroy()

        Button(frame, text="Save settings", command=save).pack(fill=X, pady=(12, 0))

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
            self.status.set("Installing missing dependencies...")
            if not resolve_rclone_path():
                install_rclone()
            if os.name == "nt" and not winfsp_installed():
                install_winfsp()
            if os.name == "nt" and not ssh_installed():
                install_openssh_client()
            self.rclone = resolve_rclone_path()
            self.status.set("Dependency check complete.")
            self.check_dependencies()
        except Exception as exc:
            self.status.set("Dependency installation failed.")
            messagebox.showerror(APP_TITLE, str(exc))

    def add_config(self) -> None:
        dialog = ServerDialog(self.root, rclone=self.rclone)
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

    def edit_server(self, server: dict) -> None:
        try:
            index = self.servers.index(server)
        except ValueError:
            return
        dialog = ServerDialog(self.root, rclone=self.rclone, existing=server)
        self.root.wait_window(dialog.window)
        if dialog.result:
            self.servers[index] = dialog.result
            save_servers(self.servers)
            self.refresh_list()

    def toggle_mount(self, server: dict) -> None:
        if mount_status(server) == "mounted":
            try:
                unmount_server(server)
                self.status.set("Unmounted.")
            except Exception as exc:
                messagebox.showerror(APP_TITLE, str(exc))
        else:
            try:
                state = mount_server(server, self.rclone)
                self.status.set(f"Mounted {state['remote']} at {state['mountpoint']}")
            except Exception as exc:
                messagebox.showerror(APP_TITLE, str(exc))
        self.refresh_list()

    def open_folder(self, server: dict) -> None:
        if mount_status(server) != "mounted":
            messagebox.showinfo(APP_TITLE, "Mount this config before opening its folder.")
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
            messagebox.showerror(APP_TITLE, str(exc))

    def delete_server(self, server: dict) -> None:
        status = mount_status(server)
        name = server.get("name") or server.get("id")
        if status == "mounted":
            if not messagebox.askyesno(APP_TITLE, f"{name} is mounted. Unmount and delete this config?"):
                return
            try:
                unmount_server(server)
            except Exception as exc:
                messagebox.showerror(APP_TITLE, str(exc))
                return
        else:
            if not messagebox.askyesno(APP_TITLE, f"Delete config {name}?"):
                return
        state_file = server_state_file(server)
        if state_file.exists() and mount_status(server) != "mounted":
            state_file.unlink(missing_ok=True)
        self.servers = [item for item in self.servers if item is not server and item.get("id") != server.get("id")]
        save_servers(self.servers)
        self.status.set(f"Deleted {name}.")
        self.refresh_list()

    def edit_selected(self) -> None:
        selection = self.listbox.curselection()
        if not selection:
            messagebox.showinfo(APP_TITLE, "Select a server first.")
            return
        index = selection[0]
        current = self.servers[index]
        dialog = ServerDialog(self.root, rclone=self.rclone, existing=current)
        self.root.wait_window(dialog.window)
        if dialog.result:
            self.servers[index] = dialog.result
            save_servers(self.servers)
            self.refresh_list()
            self.listbox.selection_set(index)

    def delete_selected(self) -> None:
        selection = self.listbox.curselection()
        if not selection:
            return
        del self.servers[selection[0]]
        save_servers(self.servers)
        self.refresh_list()

    def mount_selected(self) -> None:
        server = self.selected()
        if not server:
            return
        try:
            state = mount_server(server, self.rclone)
            self.status.set(f"Mounted {state['remote']} at {state['mountpoint']}")
            self.refresh_list()
        except Exception as exc:
            messagebox.showerror(APP_TITLE, str(exc))

    def unmount_selected(self) -> None:
        server = self.selected()
        if not server:
            return
        try:
            unmount_server(server)
            self.status.set("Unmounted.")
            self.refresh_list()
        except Exception as exc:
            messagebox.showerror(APP_TITLE, str(exc))

    def enable_startup_selected(self) -> None:
        server = self.selected()
        if not server:
            return
        try:
            enable_startup(server)
            self.status.set("Startup mount enabled.")
        except Exception as exc:
            messagebox.showerror(APP_TITLE, str(exc))

    def disable_startup_selected(self) -> None:
        server = self.selected()
        if not server:
            return
        try:
            disable_startup(server)
            self.status.set("Startup mount disabled.")
        except Exception as exc:
            messagebox.showerror(APP_TITLE, str(exc))


class ServerDialog:
    def __init__(self, root: Tk, *, rclone: str, existing: dict | None = None):
        self.result = None
        self.rclone = rclone
        self.existing = existing or {}
        existing_source = self.existing.get("source")
        if not existing_source and self.existing.get("mode") == "ssh_config":
            existing_source = "ssh_config"
        self.source = StringVar(value=existing_source or "ssh_config")
        self.auth = StringVar(value=self.existing.get("auth", "key"))
        self.values: dict[str, Entry] = {}
        self.window = Toplevel(root)
        self.window.title("Edit config" if existing else "Add config")
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
        Label(frame, text="Remote path", width=14, anchor="w").pack(side=LEFT)
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
        Label(source_frame, text="Source", width=14, anchor="w").pack(side=LEFT)
        ttk.Radiobutton(source_frame, text="SSH config", variable=self.source, value="ssh_config", command=self.on_source_changed).pack(side=LEFT)
        ttk.Radiobutton(source_frame, text="Manual", variable=self.source, value="manual", command=self.on_source_changed).pack(side=LEFT)

        hosts = list_ssh_config_hosts()
        host_default = self.existing.get("host_alias") or (hosts[0] if hosts else "")
        self.host_combo = self.row_combo("SSH Host", "host_alias", hosts, host_default)
        self.host_combo.bind("<<ComboboxSelected>>", self.on_ssh_host_selected)

        self.row("Name", "name", self.existing.get("name", ""))
        self.row("IP / Host", "host", self.existing.get("host", ""))
        self.row("User", "user", self.existing.get("user", ""))
        self.row("Port", "port", str(self.existing.get("port") or "22"))

        auth_frame = Frame(self.form, padx=10, pady=4)
        auth_frame.pack(fill=X)
        Label(auth_frame, text="Auth", width=14, anchor="w").pack(side=LEFT)
        ttk.Radiobutton(auth_frame, text="Key", variable=self.auth, value="key").pack(side=LEFT)
        ttk.Radiobutton(auth_frame, text="Password", variable=self.auth, value="password").pack(side=LEFT)
        self.row("Key file", "key_file", self.existing.get("key_file", ""), browse=True)
        self.row("Key passphrase", "key_passphrase", secret=True)
        self.row("Password", "password", secret=True)

        self.row_remote_path(self.existing.get("remote_path", ""))
        self.row_combo("Mountpoint", "mountpoint", mountpoint_choices(), self.existing.get("mountpoint") or "Auto")

        buttons = Frame(self.form, padx=10, pady=10)
        buttons.pack(fill=X)
        Button(buttons, text="Save", command=self.save).pack(side=RIGHT)
        Button(buttons, text="Cancel", command=self.window.destroy).pack(side=RIGHT, padx=6)

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
            messagebox.showerror(APP_TITLE, "Name is required.")
            return
        host = self.get("host")
        user = self.get("user")
        if not host or not user:
            messagebox.showerror(APP_TITLE, "IP/Host and user are required.")
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
                messagebox.showerror(APP_TITLE, "Password is required.")
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
