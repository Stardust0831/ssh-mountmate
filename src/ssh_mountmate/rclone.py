from __future__ import annotations

import shutil
import subprocess
import platform
import os
import stat
import hashlib
import json
import tempfile
import urllib.request
import zipfile
from pathlib import Path

from .paths import legacy_managed_bin_dirs, managed_bin_dir
from .platforms import current_platform


def bundled_rclone_candidates(app_root: Path) -> list[Path]:
    platform_info = current_platform()
    binary = platform_info.rclone_binary
    return [
        app_root / "bin" / binary,
        app_root / "resources" / "bin" / binary,
    ]


def managed_rclone_path() -> Path:
    return managed_bin_dir() / current_platform().rclone_binary


def managed_rclone_candidates() -> list[Path]:
    binary = current_platform().rclone_binary
    return [managed_rclone_path(), *[path / binary for path in legacy_managed_bin_dirs()]]


def file_sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def materialized_bundled_rclone_path(source: Path) -> Path:
    suffix = ".exe" if current_platform().system == "Windows" else ""
    return managed_bin_dir() / f"rclone-{file_sha256(source)[:16]}{suffix}"


def materialized_bundled_rclone_name(path: Path) -> bool:
    name = path.name
    if current_platform().system == "Windows":
        if not name.casefold().endswith(".exe"):
            return False
        name = name[:-4]
    token = name.removeprefix("rclone-")
    return len(token) == 16 and token != name and all(ch in "0123456789abcdef" for ch in token.casefold())


def running_process_command_lines() -> list[str]:
    system = current_platform().system
    if system == "Windows":
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
                creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0),
                timeout=3,
            )
        except Exception:
            return []
        try:
            data = json.loads(result.stdout.strip() or "[]")
        except json.JSONDecodeError:
            return []
        if isinstance(data, dict):
            data = [data]
        return [str(item.get("CommandLine") or "") for item in data if isinstance(item, dict)]

    proc = Path("/proc")
    if system == "Linux" and proc.exists():
        commands: list[str] = []
        for cmdline in proc.glob("[0-9]*/cmdline"):
            try:
                raw = cmdline.read_bytes()
            except OSError:
                continue
            if b"rclone" in raw:
                commands.append(raw.replace(b"\x00", b" ").decode(errors="ignore"))
        return commands

    try:
        result = subprocess.run(
            ["ps", "-axo", "command="],
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            timeout=3,
        )
    except Exception:
        return []
    return [line for line in result.stdout.splitlines() if "rclone" in line]


def command_references_path(command: str, path: Path) -> bool:
    text = command.casefold() if current_platform().system == "Windows" else command
    candidates = [str(path), str(path.resolve(strict=False))]
    for candidate in candidates:
        needle = candidate.casefold() if current_platform().system == "Windows" else candidate
        if needle in text:
            return True
    return False


def cleanup_managed_bundled_rclones(current: Path) -> None:
    bin_dir = managed_bin_dir()
    if not bin_dir.exists():
        return
    commands = running_process_command_lines()
    for candidate in bin_dir.iterdir():
        if candidate == current or not candidate.is_file() or not materialized_bundled_rclone_name(candidate):
            continue
        if any(command_references_path(command, candidate) for command in commands):
            continue
        try:
            candidate.unlink()
        except OSError:
            pass


def materialize_bundled_rclone(source: Path) -> Path:
    target = materialized_bundled_rclone_path(source)
    if target.exists() and target.stat().st_size == source.stat().st_size:
        cleanup_managed_bundled_rclones(target)
        return target
    target.parent.mkdir(parents=True, exist_ok=True)
    temp = target.with_name(f".{target.name}.{os.getpid()}.tmp")
    shutil.copy2(source, temp)
    if current_platform().system != "Windows":
        mode = temp.stat().st_mode
        temp.chmod(mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)
    temp.replace(target)
    cleanup_managed_bundled_rclones(target)
    return target


def rclone_download_arch(machine: str | None = None) -> str:
    value = (machine or platform.machine()).lower()
    if value in {"x86_64", "amd64"}:
        return "amd64"
    if value in {"arm64", "aarch64"}:
        return "arm64"
    if value in {"i386", "i686", "x86"}:
        return "386"
    return "amd64"


def common_rclone_paths(system: str | None = None) -> list[Path]:
    platform_name = system or platform.system()
    if platform_name == "Windows":
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


def _split_path(value: str) -> list[str]:
    return [part for part in value.split(os.pathsep) if part]


def _read_path_file(path: Path) -> list[str]:
    try:
        return [line.strip() for line in path.read_text(encoding="utf-8", errors="ignore").splitlines() if line.strip() and not line.strip().startswith("#")]
    except OSError:
        return []


def system_path_entries(system: str | None = None) -> list[str]:
    platform_name = system or platform.system()
    entries: list[str] = []

    if platform_name in {"Darwin", "Linux"}:
        entries.extend(_read_path_file(Path("/etc/paths")))
        paths_dir = Path("/etc/paths.d")
        if paths_dir.exists():
            for path_file in sorted(paths_dir.iterdir()):
                if path_file.is_file():
                    entries.extend(_read_path_file(path_file))

    if platform_name == "Darwin" and Path("/usr/libexec/path_helper").exists():
        try:
            result = subprocess.run(
                ["/usr/libexec/path_helper", "-s"],
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.DEVNULL,
                check=False,
                timeout=3,
            )
            for chunk in result.stdout.replace(";", "\n").splitlines():
                chunk = chunk.strip()
                if chunk.startswith("PATH="):
                    entries.extend(_split_path(chunk.removeprefix("PATH=").strip('"')))
        except Exception:
            pass

    shell = os.environ.get("SHELL")
    if platform_name in {"Darwin", "Linux"} and shell and Path(shell).exists():
        try:
            result = subprocess.run(
                [shell, "-lc", 'printf "%s" "$PATH"'],
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.DEVNULL,
                check=False,
                timeout=3,
            )
            entries.extend(_split_path(result.stdout))
        except Exception:
            pass

    unique: list[str] = []
    seen: set[str] = set()
    for entry in entries:
        if entry not in seen:
            seen.add(entry)
            unique.append(entry)
    return unique


def augment_process_path() -> None:
    current = _split_path(os.environ.get("PATH", ""))
    merged: list[str] = []
    seen: set[str] = set()
    for entry in current + system_path_entries():
        if entry not in seen:
            seen.add(entry)
            merged.append(entry)
    if merged:
        os.environ["PATH"] = os.pathsep.join(merged)


def resolve_rclone(app_root: Path, configured_path: str = "") -> str:
    if configured_path:
        path = Path(configured_path).expanduser()
        if path.exists():
            return str(path)
    bundled: Path | None = None
    for candidate in bundled_rclone_candidates(app_root):
        if candidate.exists():
            bundled = candidate
            try:
                return str(materialize_bundled_rclone(candidate))
            except OSError:
                continue
    for managed in managed_rclone_candidates():
        if managed.exists():
            return str(managed)
    augment_process_path()
    found = shutil.which(current_platform().rclone_binary) or shutil.which("rclone")
    if found:
        return found
    for candidate in common_rclone_paths():
        if candidate.exists():
            return str(candidate)
    if bundled:
        return str(bundled)
    return ""


def rclone_version(rclone_path: str) -> str:
    if not rclone_path:
        return "missing"
    try:
        result = subprocess.run(
            [rclone_path, "version"],
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            check=False,
        )
    except OSError:
        return "missing"
    first_line = result.stdout.splitlines()[0] if result.stdout else ""
    return first_line or "unknown"


def rclone_download_url(version: str = "current", system: str | None = None, arch: str = "amd64") -> str:
    platform_name = system or current_platform().system
    if platform_name == "Windows":
        target = f"rclone-{version}-windows-{arch}.zip"
    elif platform_name == "Darwin":
        target = f"rclone-{version}-osx-{arch}.zip"
    else:
        target = f"rclone-{version}-linux-{arch}.zip"
    return f"https://downloads.rclone.org/{target}"


def install_rclone_to(target_dir: Path) -> Path:
    platform_info = current_platform()
    binary = platform_info.rclone_binary
    arch = rclone_download_arch()
    url = rclone_download_url(system=platform_info.system, arch=arch)
    target = target_dir / binary
    target.parent.mkdir(parents=True, exist_ok=True)

    with tempfile.TemporaryDirectory(prefix="ssh-mountmate-rclone-") as temp_name:
        temp = Path(temp_name)
        archive = temp / "rclone.zip"
        urllib.request.urlretrieve(url, archive)
        with zipfile.ZipFile(archive) as zf:
            members = [member for member in zf.namelist() if Path(member).name == binary and not member.endswith("/")]
            if not members:
                raise RuntimeError(f"Downloaded rclone archive did not contain {binary}: {url}")
            extracted = Path(zf.extract(members[0], temp))
            shutil.copy2(extracted, target)

    if platform_info.system != "Windows":
        mode = target.stat().st_mode
        target.chmod(mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)
    return target


def install_managed_rclone() -> Path:
    return install_rclone_to(managed_bin_dir())


LINUX_DEPENDENCY_COMMANDS = [
    {
        "label": "Debian family (Debian, Ubuntu, Linux Mint, Pop!_OS)",
        "ids": {"debian", "ubuntu", "linuxmint", "pop"},
        "likes": {"debian", "ubuntu"},
        "install": "sudo apt update && sudo apt install -y",
        "fuse": "fuse3",
        "ssh": "openssh-client",
    },
    {
        "label": "Fedora/RHEL family (Fedora, RHEL, CentOS Stream, Rocky Linux, AlmaLinux)",
        "ids": {"fedora", "rhel", "centos", "rocky", "almalinux"},
        "likes": {"fedora", "rhel", "centos"},
        "install": "sudo dnf install -y",
        "fuse": "fuse3",
        "ssh": "openssh-clients",
    },
    {
        "label": "Arch family (Arch Linux, Manjaro, EndeavourOS)",
        "ids": {"arch", "manjaro", "endeavouros"},
        "likes": {"arch"},
        "install": "sudo pacman -S --needed",
        "fuse": "fuse3",
        "ssh": "openssh",
    },
    {
        "label": "openSUSE/SUSE family (openSUSE Leap, Tumbleweed, SLES)",
        "ids": {"opensuse", "opensuse-leap", "opensuse-tumbleweed", "sles"},
        "likes": {"suse", "opensuse"},
        "install": "sudo zypper install -y",
        "fuse": "fuse3",
        "ssh": "openssh",
    },
]


def linux_os_release(path: Path = Path("/etc/os-release")) -> dict[str, str]:
    try:
        lines = path.read_text(encoding="utf-8", errors="ignore").splitlines()
    except OSError:
        return {}
    data: dict[str, str] = {}
    for line in lines:
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, value = line.split("=", 1)
        data[key] = value.strip().strip('"').strip("'")
    return data


def linux_dependency_command(item: dict, missing: set[str] | None = None) -> str:
    packages: list[str] = []
    if missing is None or "FUSE" in missing:
        packages.append(str(item["fuse"]))
    if missing is None or "OpenSSH" in missing:
        packages.append(str(item["ssh"]))
    return f"{item['install']} {' '.join(packages)}"


def preferred_linux_dependency_command(missing: set[str] | None = None) -> tuple[str, str] | None:
    release = linux_os_release()
    distro_id = release.get("ID", "").lower()
    id_like = set(release.get("ID_LIKE", "").lower().split())
    for item in LINUX_DEPENDENCY_COMMANDS:
        if distro_id in item["ids"] or id_like.intersection(item["likes"]):
            return str(item["label"]), linux_dependency_command(item, missing)
    return None


def normalized_missing_dependencies(missing: list[str] | set[str] | None) -> set[str] | None:
    if missing is None:
        return None
    normalized: set[str] = set()
    for item in missing:
        key = str(item).lower()
        if key == "rclone":
            normalized.add("rclone")
        elif key in {"winfsp", "macfuse", "fuse"}:
            normalized.add({"winfsp": "WinFsp", "macfuse": "macFUSE", "fuse": "FUSE"}[key])
        elif key in {"openssh", "ssh", "openssh client"}:
            normalized.add("OpenSSH")
    return normalized


def linux_install_commands(missing: set[str] | None = None) -> list[str]:
    commands: list[str] = []
    if missing is None or "rclone" in missing:
        commands.extend(
            [
                "rclone:",
                "curl https://rclone.org/install.sh | sudo bash",
                "or use your distro package manager, for example: sudo apt install rclone",
                f"Manual zip: {rclone_download_url(system='Linux')}",
                "",
            ]
        )
    if missing is None or {"FUSE", "OpenSSH"}.intersection(missing):
        label = "FUSE and OpenSSH:"
        if missing is not None and "FUSE" in missing and "OpenSSH" not in missing:
            label = "FUSE:"
        elif missing is not None and "OpenSSH" in missing and "FUSE" not in missing:
            label = "OpenSSH Client:"
        commands.append(label)
        preferred = preferred_linux_dependency_command(missing) if platform.system() == "Linux" else None
        if preferred:
            distro_label, command = preferred
            commands.extend(["Recommended for this system:", distro_label, command, ""])
        commands.append("All common distro commands:")
        for item in LINUX_DEPENDENCY_COMMANDS:
            commands.extend([str(item["label"]) + ":", linux_dependency_command(item, missing)])
    return commands


def manual_install_commands(missing: list[str] | set[str] | None = None) -> dict[str, list[str]]:
    missing_set = normalized_missing_dependencies(missing)
    include_all = missing_set is None
    return {
        "Windows": [line for key, lines in [
            ("rclone", [
            "rclone:",
            f"Download and unzip: {rclone_download_url(system='Windows')}",
            "Place rclone.exe on PATH or next to SSHMountMate.exe.",
            "Optional winget fallback: winget install --id Rclone.Rclone -e",
            "",
            ]),
            ("WinFsp", [
            "WinFsp:",
            "Download the installer from: https://winfsp.dev/rel/",
            "Optional winget command: winget install --id WinFsp.WinFsp -e",
            "",
            ]),
            ("OpenSSH", [
            "OpenSSH Client:",
            'powershell -NoProfile -ExecutionPolicy Bypass -Command "Add-WindowsCapability -Online -Name OpenSSH.Client~~~~0.0.1.0"',
            ]),
        ] if include_all or key in missing_set for line in lines],
        "macOS": [line for key, lines in [
            ("rclone", [
            "rclone:",
            "Do not use the Homebrew rclone package for mounts; Homebrew rclone cannot run rclone mount on macOS.",
            "curl https://rclone.org/install.sh | sudo bash",
            f"Manual zip: {rclone_download_url(system='Darwin')}",
            "",
            ]),
            ("macFUSE", [
            "macFUSE:",
            "Install macFUSE with Homebrew Cask: brew install --cask macfuse",
            "If macFUSE asks for approval, enable it in System Settings -> Privacy & Security, then retry.",
            "",
            ]),
            ("OpenSSH", [
            "OpenSSH Client:",
            "OpenSSH is normally included with macOS. If ssh is missing, run: xcode-select --install",
            ]),
        ] if include_all or key in missing_set for line in lines],
        "Linux": linux_install_commands(missing_set),
    }


def manual_install_text(missing: list[str] | set[str] | None = None) -> str:
    lines = ["manual dependency install options", ""]
    commands_by_system = manual_install_commands(missing)
    if missing is not None:
        current = platform.system()
        current = "macOS" if current == "Darwin" else current
        commands_by_system = {current: commands_by_system.get(current, [])}
    for system, commands in commands_by_system.items():
        if not commands:
            continue
        lines.append(f"{system}:")
        for command in commands:
            lines.append(f"  {command}" if command else "")
        lines.append("")
    return "\n".join(lines).rstrip()
