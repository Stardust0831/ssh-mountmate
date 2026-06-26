#!/usr/bin/env python3
import argparse
import configparser
import json
import os
import shlex
import shutil
import subprocess
import sys
import time
from pathlib import Path


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


def pid_file(host: str) -> Path:
    return app_state_dir() / f"{host}.json"


def default_known_hosts_file() -> Path:
    return Path.home() / ".ssh" / "known_hosts"


def winfsp_paths() -> list[Path]:
    if not is_windows():
        return []
    roots = [
        os.environ.get("ProgramFiles(x86)", "C:\\Program Files (x86)"),
        os.environ.get("ProgramFiles", "C:\\Program Files"),
    ]
    return [Path(root) / "WinFsp" for root in roots]


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

    path_rclone = shutil.which("rclone")
    if path_rclone:
        return path_rclone

    die("rclone not found. Use the packaged release with bin/rclone, or set RSSHMOUNT_RCLONE.")


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
    known_hosts = default_known_hosts_file()
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

    known_hosts = first_usable_path(config.get("userknownhostsfile", []), must_exist=True)
    if not known_hosts:
        known_hosts_path = default_known_hosts_file()
        if known_hosts_path.exists():
            known_hosts = str(known_hosts_path)
    if known_hosts:
        parser.set(host, "known_hosts_file", known_hosts)


def ensure_rclone_remote(host: str, ssh_config: str | None, transport: str) -> Path:
    validate_host(host)
    conf_path = rclone_config_path()
    conf_path.parent.mkdir(parents=True, exist_ok=True)
    resolved_ssh = read_ssh_config(host, ssh_config)
    chosen_transport = choose_transport(transport, resolved_ssh)

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


def default_mountpoint(host: str) -> Path:
    if is_windows():
        for letter in "ZYXWVUTSRQPONMLKJIHGFED":
            drive = f"{letter}:"
            if not Path(drive + "\\").exists():
                return Path(drive)
        die("no free drive letter found; pass a mountpoint such as X:")
    return Path.home() / "mnt" / host


def is_windows_drive(value: str) -> bool:
    return len(value) in (2, 3) and value[1] == ":" and value[0].isalpha()


def prepare_mountpoint(mountpoint: Path) -> None:
    value = str(mountpoint)
    if is_windows():
        if value == "*" or is_windows_drive(value):
            return
        parent = mountpoint.parent
        if not parent.exists():
            die(f"mountpoint parent does not exist: {parent}")
        if mountpoint.exists():
            die(f"Windows directory mountpoint must not already exist: {mountpoint}")
        return
    mountpoint.mkdir(parents=True, exist_ok=True)


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
        log = log_path.open("ab")
        creationflags = (
            getattr(subprocess, "DETACHED_PROCESS", 0)
            | getattr(subprocess, "CREATE_NEW_PROCESS_GROUP", 0)
            | getattr(subprocess, "CREATE_NO_WINDOW", 0)
        )
        try:
            proc = subprocess.Popen(cmd, stdout=log, stderr=subprocess.STDOUT, creationflags=creationflags)
        except FileNotFoundError:
            die(f"command not found: {cmd[0]}")

        time.sleep(2)
        if proc.poll() is not None:
            die(f"mount failed; see log: {log_path}", proc.returncode or 1)

        state = {
            "pid": proc.pid,
            "host": args.host,
            "remote": remote_spec(args.host, args.remote_path),
            "mountpoint": str(mountpoint),
            "log": str(log_path),
        }
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
