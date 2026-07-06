import configparser
import ctypes
import os
import shlex
import shutil
import subprocess
import time
from contextlib import contextmanager
from pathlib import Path


APP = "rsshmount"


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
def exclusive_file_lock(path: Path, *, timeout: float = 120.0, description: str = "operation"):
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
    return exclusive_file_lock(rclone_config_lock_path(), timeout=180.0, description="rclone config")


def server_operation_file_lock(server_id: str):
    return exclusive_file_lock(server_operation_lock_path(server_id), timeout=180.0, description=f"{server_id} mount")


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


def validate_host(host: str) -> None:
    allowed = set("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789._-")
    if not host or any(ch not in allowed for ch in host):
        raise ValueError("Host must be a simple SSH alias using only letters, digits, dot, underscore, or dash.")


def run(cmd, *, check=True, capture=False):
    creationflags = getattr(subprocess, "CREATE_NO_WINDOW", 0) if is_windows() else 0
    return subprocess.run(
        cmd,
        check=check,
        text=True,
        stdout=subprocess.PIPE if capture else None,
        stderr=subprocess.PIPE if capture else None,
        creationflags=creationflags,
    )


def ssh_base_args(ssh_config: str | None) -> list[str]:
    args = ["ssh"]
    if ssh_config:
        args.extend(["-F", str(Path(ssh_config).expanduser())])
    return args


def ssh_cmd_for_rclone(host: str, ssh_config: str | None) -> str:
    parts = ssh_base_args(ssh_config) + ["-o", "BatchMode=yes", host]
    return " ".join(shlex.quote(part) for part in parts)


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


def known_hosts_for_external_remote(host: str, ssh_config: str | None) -> Path | None:
    port = first_ssh_value(read_ssh_config(host, ssh_config), "port", "22") if ssh_config else "22"
    known_hosts = update_app_known_hosts(host, port) or default_known_hosts_file()
    return known_hosts if known_hosts.exists() else None


def write_external_remote(parser, host: str, ssh_config: str | None, known_hosts: Path | None = None) -> None:
    parser.set(host, "type", "sftp")
    parser.set(host, "ssh", ssh_cmd_for_rclone(host, ssh_config))
    parser.set(host, "shell_type", "unix")
    parser.set(host, "disable_hashcheck", "true")
    if known_hosts:
        parser.set(host, "known_hosts_file", str(known_hosts))


def known_hosts_for_native_remote(host: str, config: dict[str, list[str]]) -> str:
    host_name = first_ssh_value(config, "hostname", host)
    port = first_ssh_value(config, "port", "22")
    known_hosts = update_app_known_hosts(host_name, port)
    if not known_hosts:
        known_hosts = first_usable_path(config.get("userknownhostsfile", []), must_exist=True)
        if not known_hosts:
            known_hosts_path = default_known_hosts_file()
            if known_hosts_path.exists():
                known_hosts = str(known_hosts_path)
    return str(known_hosts or "")


def write_native_remote(parser, host: str, config: dict[str, list[str]], known_hosts: str = "") -> None:
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

    if known_hosts:
        parser.set(host, "known_hosts_file", str(known_hosts))


def ensure_rclone_remote(host: str, ssh_config: str | None, transport: str) -> Path:
    validate_host(host)
    conf_path = rclone_config_path()
    conf_path.parent.mkdir(parents=True, exist_ok=True)
    resolved_ssh = read_ssh_config(host, ssh_config)
    chosen_transport = choose_transport(transport, resolved_ssh)
    known_hosts = (
        known_hosts_for_native_remote(host, resolved_ssh)
        if chosen_transport == "native"
        else known_hosts_for_external_remote(host, ssh_config)
    )

    with rclone_config_file_lock():
        parser = configparser.RawConfigParser()
        parser.optionxform = str
        parser.read(conf_path)

        if parser.has_section(host):
            parser.remove_section(host)
        parser.add_section(host)

        if chosen_transport == "native":
            write_native_remote(parser, host, resolved_ssh, str(known_hosts or ""))
        else:
            write_external_remote(parser, host, ssh_config, known_hosts)

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
        raise RuntimeError("no free drive letter found; pass a mountpoint such as X:")
    return home_mountpoint(host)


def home_mountpoint(host: str) -> Path:
    return Path.home() / "mnt" / host


def is_windows_drive(value: str) -> bool:
    return len(value) in (2, 3) and value[1] == ":" and value[0].isalpha()
