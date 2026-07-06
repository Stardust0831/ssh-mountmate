#!/usr/bin/env python3
import argparse
import configparser
import ctypes
import json
import os
import shlex
import shutil
import subprocess
import sys
import time
from contextlib import contextmanager
from pathlib import Path

from . import mount_process
from .rclone import augment_process_path


APP = "rsshmount"
DEFAULT_REMOTE_PATH = ""


def is_windows() -> bool:
    return os.name == "nt"


def xdg_config_home() -> Path:
    if is_windows():
        return Path(os.environ.get("APPDATA", Path.home() / "AppData" / "Roaming"))
    return Path(os.environ.get("XDG_CONFIG_HOME", Path.home() / ".config"))


def xdg_cache_home() -> Path:
    if is_windows():
        return Path(os.environ.get("LOCALAPPDATA", Path.home() / "AppData" / "Local")) / APP / "Cache"
    return Path(os.environ.get("XDG_CACHE_HOME", Path.home() / ".cache"))


def xdg_state_home() -> Path:
    if is_windows():
        return Path(os.environ.get("LOCALAPPDATA", Path.home() / "AppData" / "Local")) / APP / "State"
    return Path(os.environ.get("XDG_STATE_HOME", Path.home() / ".local" / "state"))


def app_config_dir() -> Path:
    return xdg_config_home() / APP


def rclone_config_path() -> Path:
    return app_config_dir() / "rclone.conf"


def app_cache_dir(host: str) -> Path:
    if is_windows():
        return xdg_cache_home() / host
    return xdg_cache_home() / APP / host


def app_state_dir() -> Path:
    if is_windows():
        return xdg_state_home()
    return xdg_state_home() / APP


def app_lock_dir() -> Path:
    return app_state_dir() / "locks"


def pid_file(host: str) -> Path:
    return app_state_dir() / f"{host}.json"


def lock_name(value: str) -> str:
    cleaned = "".join(ch if ch.isalnum() or ch in "._-" else "_" for ch in value)
    return cleaned.strip("._-") or "default"


@contextmanager
def exclusive_file_lock(path: Path, *, timeout: float = 45.0, description: str = "operation"):
    path.parent.mkdir(parents=True, exist_ok=True)
    deadline = time.monotonic() + timeout
    locked = False
    with path.open("a+b") as handle:
        handle.seek(0, os.SEEK_END)
        if handle.tell() == 0:
            handle.write(b"0")
            handle.flush()
        handle.seek(0)
        while not locked:
            try:
                if is_windows():
                    import msvcrt

                    msvcrt.locking(handle.fileno(), msvcrt.LK_NBLCK, 1)
                else:
                    import fcntl

                    fcntl.flock(handle.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
                locked = True
            except OSError as exc:
                if time.monotonic() >= deadline:
                    raise RuntimeError(f"Timed out waiting for {description} lock: {path}") from exc
                time.sleep(0.1)
        try:
            yield
        finally:
            if is_windows():
                import msvcrt

                handle.seek(0)
                msvcrt.locking(handle.fileno(), msvcrt.LK_UNLCK, 1)
            else:
                import fcntl

                fcntl.flock(handle.fileno(), fcntl.LOCK_UN)


def rclone_config_lock_path() -> Path:
    return app_config_dir() / "rclone.conf.lock"


def server_operation_lock_path(server_id: str) -> Path:
    return app_lock_dir() / f"{lock_name(server_id)}.lock"


def rclone_config_file_lock():
    return exclusive_file_lock(rclone_config_lock_path(), description="rclone config")


def server_operation_file_lock(server_id: str):
    return exclusive_file_lock(server_operation_lock_path(server_id), description=f"{server_id} mount")


def default_known_hosts_file() -> Path:
    return Path.home() / ".ssh" / "known_hosts"


def app_known_hosts_file() -> Path:
    return app_config_dir() / "known_hosts"


def known_hosts_marker(host: str, port: str | int) -> str:
    port_value = str(port or "22")
    return f"[{host}]:{port_value}" if port_value != "22" else host


def known_hosts_line_matches(line: str, marker: str) -> bool:
    stripped = line.strip()
    if not stripped or stripped.startswith("#"):
        return False
    if stripped.startswith("@"):
        parts = stripped.split(None, 2)
        if len(parts) < 2:
            return False
        hosts = parts[1]
    else:
        hosts = stripped.split(None, 1)[0]
    return marker in hosts.split(",")


def scan_host_keys(host: str, port: str | int) -> list[str]:
    keyscan = shutil.which("ssh-keyscan")
    if not keyscan or not host:
        return []
    cmd = [
        keyscan,
        "-T",
        "8",
        "-p",
        str(port or "22"),
        "-t",
        "rsa,ecdsa,ed25519",
        host,
    ]
    try:
        result = subprocess.run(
            cmd,
            check=False,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0) if is_windows() else 0,
            timeout=12,
        )
    except (OSError, subprocess.TimeoutExpired):
        return []
    lines: list[str] = []
    seen: set[str] = set()
    for raw_line in result.stdout.splitlines():
        line = raw_line.strip()
        if not line or line.startswith("#"):
            continue
        parts = line.split()
        if len(parts) >= 3 and parts[1].startswith(("ssh-", "ecdsa-")) and line not in seen:
            lines.append(line)
            seen.add(line)
    return lines


def update_app_known_hosts(host: str, port: str | int) -> Path | None:
    scanned = scan_host_keys(host, port)
    if not scanned:
        return None
    path = app_known_hosts_file()
    marker = known_hosts_marker(host, port)
    try:
        path.parent.mkdir(parents=True, exist_ok=True)
    except OSError:
        return None
    try:
        existing = path.read_text(encoding="utf-8", errors="ignore").splitlines()
    except OSError:
        existing = []
    kept = [line for line in existing if not known_hosts_line_matches(line, marker)]
    try:
        path.write_text("\n".join([*kept, *scanned, ""]), encoding="utf-8")
        path.chmod(0o600)
    except OSError:
        return None
    return path


def winfsp_paths() -> list[Path]:
    if not is_windows():
        return []
    paths: list[Path] = []
    try:
        import winreg

        uninstall_keys = [
            r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall",
            r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall",
        ]
        for uninstall_key in uninstall_keys:
            try:
                with winreg.OpenKey(winreg.HKEY_LOCAL_MACHINE, uninstall_key) as key:
                    count = winreg.QueryInfoKey(key)[0]
                    for index in range(count):
                        try:
                            subkey_name = winreg.EnumKey(key, index)
                            with winreg.OpenKey(key, subkey_name) as subkey:
                                display_name = winreg.QueryValueEx(subkey, "DisplayName")[0]
                                if "WinFsp" not in str(display_name):
                                    continue
                                install_location = winreg.QueryValueEx(subkey, "InstallLocation")[0]
                                if install_location:
                                    paths.append(Path(winreg.ExpandEnvironmentStrings(str(install_location))))
                        except OSError:
                            continue
            except OSError:
                continue
    except ImportError:
        pass

    roots = [
        os.environ.get("ProgramFiles(x86)", "C:\\Program Files (x86)"),
        os.environ.get("ProgramFiles", "C:\\Program Files"),
    ]
    paths.extend(Path(root) / "WinFsp" for root in roots)

    unique: list[Path] = []
    seen: set[str] = set()
    for path in paths:
        key = str(path).casefold()
        if key not in seen:
            seen.add(key)
            unique.append(path)
    return unique


def find_winfsp() -> Path | None:
    for root in winfsp_paths():
        if root.exists():
            return root
    return None


def require_winfsp() -> None:
    if is_windows() and not find_winfsp():
        die("WinFsp not found. Install it from https://winfsp.dev/rel/ and retry mount.")


def validate_host(host: str) -> None:
    allowed = set("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789._-")
    if not host or any(ch not in allowed for ch in host):
        die("Host must be a simple SSH alias using only letters, digits, dot, underscore, or dash.")


def die(message: str, code: int = 1) -> None:
    print(f"{APP}: {message}", file=sys.stderr)
    raise SystemExit(code)


def run(cmd, *, check=True, capture=False):
    creationflags = getattr(subprocess, "CREATE_NO_WINDOW", 0) if is_windows() else 0
    try:
        return subprocess.run(
            cmd,
            check=check,
            text=True,
            stdout=subprocess.PIPE if capture else None,
            stderr=subprocess.PIPE if capture else None,
            creationflags=creationflags,
        )
    except FileNotFoundError:
        die(f"command not found: {cmd[0]}")
    except subprocess.CalledProcessError as exc:
        if capture:
            if exc.stdout:
                print(exc.stdout, file=sys.stderr, end="")
            if exc.stderr:
                print(exc.stderr, file=sys.stderr, end="")
        die(f"command failed: {shlex.join(cmd)}", exc.returncode)


def bundled_rclone(cli_path: str | None) -> str:
    if cli_path:
        return cli_path

    env_path = os.environ.get("RSSHMOUNT_RCLONE")
    if env_path:
        return env_path

    script_dir = Path(__file__).resolve().parent
    exe = "rclone.exe" if is_windows() else "rclone"
    candidates = [
        script_dir / "bin" / exe,
        script_dir.parent / "bin" / exe,
    ]
    for candidate in candidates:
        if candidate.exists() and os.access(candidate, os.X_OK):
            return str(candidate)

    if not is_windows():
        augment_process_path()
    path_rclone = shutil.which("rclone")
    if path_rclone:
        return path_rclone

    for candidate in common_rclone_paths():
        if candidate.exists() and os.access(candidate, os.X_OK):
            return str(candidate)

    die("rclone not found. Use the packaged release with bin/rclone, or set RSSHMOUNT_RCLONE.")


def common_rclone_paths() -> list[Path]:
    if is_windows():
        return []
    home = Path.home()
    return [
        home / ".local" / "bin" / "rclone",
        Path("/opt/homebrew/bin/rclone"),
        Path("/usr/local/bin/rclone"),
        Path("/opt/local/bin/rclone"),
        Path("/usr/bin/rclone"),
        Path("/snap/bin/rclone"),
    ]


def ssh_base_args(ssh_config: str | None) -> list[str]:
    args = ["ssh"]
    if ssh_config:
        args.extend(["-F", str(Path(ssh_config).expanduser())])
    return args


def ssh_cmd_for_rclone(host: str, ssh_config: str | None) -> str:
    parts = ssh_base_args(ssh_config) + ["-o", "BatchMode=yes", host]
    return " ".join(shlex.quote(part) for part in parts)


def check_ssh_config(host: str, ssh_config: str | None) -> None:
    args = ssh_base_args(ssh_config) + ["-G", host]
    run(args, capture=True)


def read_ssh_config(host: str, ssh_config: str | None) -> dict[str, list[str]]:
    args = ssh_base_args(ssh_config) + ["-G", host]
    result = run(args, capture=True)
    parsed: dict[str, list[str]] = {}
    for line in result.stdout.splitlines():
        if not line.strip() or line.startswith("#") or " " not in line:
            continue
        key, value = line.split(None, 1)
        parsed.setdefault(key.lower(), []).append(value.strip())
    return parsed


def first_ssh_value(config: dict[str, list[str]], key: str, default: str = "") -> str:
    values = config.get(key.lower()) or []
    return values[0] if values else default


def usable_ssh_path(value: str) -> str:
    value = value.strip().strip('"')
    if not value or value.lower() == "none" or value == "/dev/null":
        return ""
    return str(Path(value).expanduser())


def first_usable_path(values: list[str], *, must_exist: bool = False) -> str:
    for value in values:
        for item in value.split():
            path = usable_ssh_path(item)
            if path and (not must_exist or Path(path).exists()):
                return path
    return ""


def probe_ssh(host: str, ssh_config: str | None) -> None:
    args = ssh_base_args(ssh_config) + ["-o", "BatchMode=yes", host, "true"]
    run(args)


def ssh_config_needs_external_transport(config: dict[str, list[str]]) -> bool:
    proxy_jump = first_ssh_value(config, "proxyjump", "none").lower()
    proxy_command = first_ssh_value(config, "proxycommand", "none").lower()
    return proxy_jump not in ("", "none") or proxy_command not in ("", "none")


def choose_transport(requested: str, config: dict[str, list[str]]) -> str:
    if requested != "auto":
        return requested
    if is_windows() and not ssh_config_needs_external_transport(config):
        return "native"
    return "external"


def write_external_remote(parser, host: str, ssh_config: str | None) -> None:
    parser.set(host, "type", "sftp")
    parser.set(host, "ssh", ssh_cmd_for_rclone(host, ssh_config))
    parser.set(host, "shell_type", "unix")
    parser.set(host, "disable_hashcheck", "true")
    port = first_ssh_value(read_ssh_config(host, ssh_config), "port", "22") if ssh_config else "22"
    known_hosts = update_app_known_hosts(host, port) or default_known_hosts_file()
    if known_hosts.exists():
        parser.set(host, "known_hosts_file", str(known_hosts))


def write_native_remote(parser, host: str, config: dict[str, list[str]]) -> None:
    parser.set(host, "type", "sftp")
    parser.set(host, "host", first_ssh_value(config, "hostname", host))
    parser.set(host, "user", first_ssh_value(config, "user", os.environ.get("USERNAME", "")))
    parser.set(host, "port", first_ssh_value(config, "port", "22"))
    parser.set(host, "shell_type", "unix")
    parser.set(host, "disable_hashcheck", "true")

    key_file = first_usable_path(config.get("identityfile", []), must_exist=True)
    if key_file:
        parser.set(host, "key_file", key_file)
    else:
        parser.set(host, "key_use_agent", "true")

    host_name = first_ssh_value(config, "hostname", host)
    port = first_ssh_value(config, "port", "22")
    known_hosts = update_app_known_hosts(host_name, port)
    if not known_hosts:
        known_hosts = first_usable_path(config.get("userknownhostsfile", []), must_exist=True)
        if not known_hosts:
            known_hosts_path = default_known_hosts_file()
            if known_hosts_path.exists():
                known_hosts = str(known_hosts_path)
    if known_hosts:
        parser.set(host, "known_hosts_file", str(known_hosts))


def ensure_rclone_remote(host: str, ssh_config: str | None, transport: str) -> Path:
    validate_host(host)
    conf_path = rclone_config_path()
    conf_path.parent.mkdir(parents=True, exist_ok=True)
    resolved_ssh = read_ssh_config(host, ssh_config)
    chosen_transport = choose_transport(transport, resolved_ssh)

    with rclone_config_file_lock():
        parser = configparser.RawConfigParser()
        parser.optionxform = str
        parser.read(conf_path)

        if parser.has_section(host):
            parser.remove_section(host)
        parser.add_section(host)

        if chosen_transport == "native":
            write_native_remote(parser, host, resolved_ssh)
        else:
            write_external_remote(parser, host, ssh_config)

        with conf_path.open("w", encoding="utf-8") as fh:
            parser.write(fh)

    return conf_path


def remote_spec(host: str, remote_path: str) -> str:
    if not remote_path:
        return f"{host}:"
    return f"{host}:{remote_path}"


def windows_drive_in_use(value: str) -> bool:
    if not is_windows() or not is_windows_drive(value):
        return False
    try:
        mask = ctypes.windll.kernel32.GetLogicalDrives()
    except Exception:
        mask = 0
    letter = value[0].upper()
    bit = 1 << (ord(letter) - ord("A"))
    if mask:
        return bool(mask & bit)
    return Path(f"{letter}:\\").exists()


def default_mountpoint(host: str) -> Path:
    if is_windows():
        for letter in "ZYXWVUTSRQPONMLKJIHGFED":
            drive = f"{letter}:"
            if not windows_drive_in_use(drive):
                return Path(drive)
        die("no free drive letter found; pass a mountpoint such as X:")
    return home_mountpoint(host)


def home_mountpoint(host: str) -> Path:
    return Path.home() / "mnt" / host


def is_windows_drive(value: str) -> bool:
    return len(value) in (2, 3) and value[1] == ":" and value[0].isalpha()


def prepare_mountpoint(mountpoint: Path) -> None:
    value = str(mountpoint)
    if is_windows():
        if value == "*":
            return
        if is_windows_drive(value):
            if windows_drive_in_use(value):
                die(f"Windows drive is already in use: {value}")
            return
        parent = mountpoint.parent
        if not parent.exists():
            die(f"mountpoint parent does not exist: {parent}")
        if mountpoint.exists():
            die(f"Windows directory mountpoint must not already exist: {mountpoint}")
        return
    mountpoint.mkdir(parents=True, exist_ok=True)


def mountpoint_ready(mountpoint: Path | str) -> bool:
    path = Path(mountpoint)
    value = str(path)
    try:
        if is_windows() and is_windows_drive(value):
            return windows_drive_in_use(value)
        if is_windows():
            return path.exists()
        return path.is_mount()
    except OSError:
        return False


def cmd_list_hosts(args) -> None:
    config = Path(args.ssh_config or Path.home() / ".ssh" / "config").expanduser()
    if not config.exists():
        die(f"SSH config not found: {config}")

    hosts: list[str] = []
    for line in config.read_text(encoding="utf-8").splitlines():
        stripped = line.strip()
        if not stripped or stripped.startswith("#"):
            continue
        words = stripped.split()
        if words and words[0].lower() == "host":
            for host in words[1:]:
                if "*" not in host and "?" not in host and "!" not in host:
                    hosts.append(host)

    for host in hosts:
        print(host)


def cmd_init(args) -> None:
    validate_host(args.host)
    check_ssh_config(args.host, args.ssh_config)
    if not args.no_probe:
        probe_ssh(args.host, args.ssh_config)
    conf_path = ensure_rclone_remote(args.host, args.ssh_config, args.transport)
    print(f"initialized {args.host} in {conf_path}")


def cmd_doctor(args) -> None:
    rclone = bundled_rclone(args.rclone)
    print(f"rclone: {rclone}", flush=True)
    run([rclone, "version"])
    print("ssh:", flush=True)
    run(["ssh", "-V"], check=False)
    if is_windows():
        found = find_winfsp()
        if found:
            print(f"WinFsp: {found}")
        else:
            die("WinFsp not found. Install it from https://winfsp.dev/rel/ before using mount.", 2)
        return
    for candidate in ["fusermount3", "fusermount", "umount"]:
        found = shutil.which(candidate)
        if found:
            print(f"{candidate}: {found}")
            break
    else:
        print("warning: no fusermount3/fusermount/umount found", file=sys.stderr)


def cmd_mount(args) -> None:
    rclone = bundled_rclone(args.rclone)
    require_winfsp()
    conf_path = ensure_rclone_remote(args.host, args.ssh_config, args.transport)
    mountpoint = Path(args.mountpoint).expanduser() if args.mountpoint else default_mountpoint(args.host)
    prepare_mountpoint(mountpoint)
    app_cache_dir(args.host).mkdir(parents=True, exist_ok=True)
    app_state_dir().mkdir(parents=True, exist_ok=True)

    cmd = [
        rclone,
        "--config",
        str(conf_path),
        "mount",
        remote_spec(args.host, args.remote_path),
        str(mountpoint),
        "--vfs-cache-mode",
        args.cache_mode,
        "--vfs-fast-fingerprint",
        "--cache-dir",
        str(app_cache_dir(args.host)),
        "--log-file",
        str(app_state_dir() / f"{args.host}.log"),
        "--dir-cache-time",
        args.dir_cache_time,
    ]

    if args.allow_other:
        cmd.append("--allow-other")
    if args.network_mode:
        cmd.append("--network-mode")
    if args.volname:
        cmd.extend(["--volname", args.volname])

    if is_windows():
        if args.foreground:
            run(cmd)
            return

        log_path = app_state_dir() / f"{args.host}.log"
        creationflags = (
            getattr(subprocess, "DETACHED_PROCESS", 0)
            | getattr(subprocess, "CREATE_NEW_PROCESS_GROUP", 0)
            | getattr(subprocess, "CREATE_NO_WINDOW", 0)
        )
        try:
            mountpoint_existed = mountpoint_ready(mountpoint)
            with log_path.open("ab") as log:
                proc = subprocess.Popen(cmd, stdout=log, stderr=subprocess.STDOUT, creationflags=creationflags)
        except FileNotFoundError:
            die(f"command not found: {cmd[0]}")

        state = {
            "pid": proc.pid,
            "host": args.host,
            "remote": remote_spec(args.host, args.remote_path),
            "mountpoint": str(mountpoint),
            "log": str(log_path),
        }
        try:
            mount_process.wait_for_mount_ready(
                proc,
                str(mountpoint),
                log_path,
                state,
                ready_before_start=mountpoint_existed,
                mountpoint_ready=lambda value: mountpoint_ready(Path(value)),
            )
        except RuntimeError as exc:
            die(str(exc), proc.returncode or 1)
        pid_file(args.host).write_text(json.dumps(state, indent=2), encoding="utf-8")
        print(f"mounted {state['remote']} at {mountpoint} with pid {proc.pid}")
        return

    if not args.foreground:
        cmd.append("--daemon")

    run(cmd)
    if not args.foreground:
        print(f"mounted {remote_spec(args.host, args.remote_path)} at {mountpoint}")


def cmd_umount(args) -> None:
    mountpoint = Path(args.mountpoint).expanduser() if args.mountpoint else default_mountpoint(args.host)
    if is_windows():
        state_path = pid_file(args.host)
        if not state_path.exists():
            die(f"pid file not found: {state_path}. Stop the rclone mount process manually.")
        state = json.loads(state_path.read_text(encoding="utf-8"))
        pid = str(state["pid"])
        result = subprocess.run(["taskkill", "/PID", pid, "/T"], text=True)
        if result.returncode != 0:
            run(["taskkill", "/PID", pid, "/T", "/F"])
        state_path.unlink(missing_ok=True)
        print(f"unmounted {state.get('mountpoint', mountpoint)}")
        return
    for tool in ["fusermount3", "fusermount"]:
        if shutil.which(tool):
            run([tool, "-u", str(mountpoint)])
            print(f"unmounted {mountpoint}")
            return
    run(["umount", str(mountpoint)])
    print(f"unmounted {mountpoint}")


def cmd_sync(args) -> None:
    rclone = bundled_rclone(args.rclone)
    conf_path = ensure_rclone_remote(args.host, args.ssh_config, args.transport)
    cmd = [
        rclone,
        "--config",
        str(conf_path),
        "sync",
        str(Path(args.source).expanduser()),
        remote_spec(args.host, args.remote_path),
        "--progress",
        "--transfers",
        str(args.transfers),
        "--checkers",
        str(args.checkers),
    ]
    if args.dry_run:
        cmd.append("--dry-run")
    run(cmd)


def cmd_status(args) -> None:
    mountpoint = Path(args.mountpoint).expanduser() if args.mountpoint else default_mountpoint(args.host)
    if is_windows():
        state_path = pid_file(args.host)
        if not state_path.exists():
            die(f"not mounted by {APP}: {args.host}", 2)
        state = json.loads(state_path.read_text(encoding="utf-8"))
        print(json.dumps(state, indent=2))
        raise SystemExit(0)
    if shutil.which("findmnt"):
        result = subprocess.run(["findmnt", "-T", str(mountpoint)], text=True)
        raise SystemExit(result.returncode)
    print(mountpoint)


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog=APP,
        description="Mount Linux servers through rclone SFTP while reusing OpenSSH config.",
    )
    parser.add_argument("--ssh-config", help="SSH config file. Defaults to OpenSSH behavior.")
    parser.add_argument("--rclone", help="Override bundled rclone path.")
    parser.add_argument(
        "--transport",
        choices=["auto", "native", "external"],
        default="auto",
        help="SFTP transport. On Windows, auto uses native rclone SSH unless ProxyJump/ProxyCommand is detected.",
    )

    sub = parser.add_subparsers(dest="command", required=True)

    p = sub.add_parser("list-hosts", help="List simple Host entries from SSH config.")
    p.set_defaults(func=cmd_list_hosts)

    p = sub.add_parser("init", help="Create or refresh the private rclone remote for a Host.")
    p.add_argument("host")
    p.add_argument("--no-probe", action="store_true", help="Skip ssh host true connectivity probe.")
    p.set_defaults(func=cmd_init)

    p = sub.add_parser("mount", help="Mount a remote path.")
    p.add_argument("host")
    p.add_argument("remote_path", nargs="?", default=DEFAULT_REMOTE_PATH)
    p.add_argument("mountpoint", nargs="?")
    p.add_argument("--foreground", action="store_true")
    p.add_argument("--allow-other", action="store_true")
    p.add_argument("--network-mode", action="store_true", help="Use rclone Windows network-drive mode.")
    p.add_argument("--volname", help="Set rclone mount volume name.")
    p.add_argument("--cache-mode", choices=["off", "minimal", "writes", "full"], default="writes")
    p.add_argument("--dir-cache-time", default="30s")
    p.set_defaults(func=cmd_mount)

    p = sub.add_parser("umount", help="Unmount a mountpoint.")
    p.add_argument("host")
    p.add_argument("mountpoint", nargs="?")
    p.set_defaults(func=cmd_umount)

    p = sub.add_parser("sync", help="Sync a local directory to a remote path.")
    p.add_argument("source")
    p.add_argument("host")
    p.add_argument("remote_path")
    p.add_argument("--dry-run", action="store_true")
    p.add_argument("--transfers", type=int, default=4)
    p.add_argument("--checkers", type=int, default=8)
    p.set_defaults(func=cmd_sync)

    p = sub.add_parser("status", help="Show mount status for the default or given mountpoint.")
    p.add_argument("host")
    p.add_argument("mountpoint", nargs="?")
    p.set_defaults(func=cmd_status)

    p = sub.add_parser("doctor", help="Check packaged dependencies.")
    p.set_defaults(func=cmd_doctor)

    return parser


def main(argv=None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    args.func(args)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
