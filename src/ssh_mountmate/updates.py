from __future__ import annotations

import json
import hashlib
import platform
import re
import shutil
import stat
import sys
import urllib.parse
import urllib.error
import urllib.request
import zipfile
from dataclasses import dataclass
from pathlib import Path
from typing import Any


REPO = "Stardust0831/ssh-mountmate"
LATEST_RELEASE_API = f"https://api.github.com/repos/{REPO}/releases/latest"
LATEST_RELEASE_PAGE = f"https://github.com/{REPO}/releases/latest"
RELEASES_PAGE = f"https://github.com/{REPO}/releases"


@dataclass(slots=True)
class ReleaseAsset:
    name: str
    url: str
    digest: str = ""
    size: int = 0


@dataclass(slots=True)
class UpdateInfo:
    current_version: str
    latest_version: str
    release_name: str
    release_url: str
    is_newer: bool
    asset: ReleaseAsset | None
    expected_asset: str
    body: str


def version_tuple(value: str) -> tuple[int, ...]:
    text = value.strip().lower()
    if text.startswith("v"):
        text = text[1:]
    numbers = re.findall(r"\d+", text)
    return tuple(int(item) for item in numbers[:4])


def version_key(value: str) -> tuple[int, int, int, int, int]:
    text = value.strip().lower()
    if text.startswith("v"):
        text = text[1:]
    match = re.match(r"(\d+)(?:\.(\d+))?(?:\.(\d+))?", text)
    release = [0, 0, 0]
    if match:
        for index, group in enumerate(match.groups()):
            if group is not None:
                release[index] = int(group)
    suffix = text[match.end():] if match else text
    stage = 4
    number = 0
    pre_match = re.search(r"(dev|a|alpha|b|beta|rc|pre|preview)[.-]?(\d*)", suffix)
    if pre_match:
        label = pre_match.group(1)
        stage = {
            "dev": 0,
            "a": 1,
            "alpha": 1,
            "b": 2,
            "beta": 2,
            "rc": 3,
            "pre": 3,
            "preview": 3,
        }.get(label, 3)
        number = int(pre_match.group(2) or "0")
    return (release[0], release[1], release[2], stage, number)


def compare_versions(left: str, right: str) -> int:
    left_key = version_key(left)
    right_key = version_key(right)
    if left_key < right_key:
        return -1
    if left_key > right_key:
        return 1
    return 0


def current_arch(machine: str | None = None) -> str:
    value = (machine or platform.machine()).lower()
    if value in {"arm64", "aarch64"}:
        return "arm64"
    return "x64"


def current_platform_name(system: str | None = None) -> str:
    value = system or platform.system()
    if value == "Windows":
        return "windows"
    if value == "Darwin":
        return "macos"
    return "linux"


def running_onedir() -> bool:
    if not getattr(sys, "frozen", False):
        return False
    bundle_root = getattr(sys, "_MEIPASS", "")
    if not bundle_root:
        return False
    bundle = Path(bundle_root).resolve()
    executable_dir = Path(sys.executable).resolve().parent
    roots = [executable_dir]
    if executable_dir.name == "MacOS" and executable_dir.parent.name == "Contents":
        roots.append(executable_dir.parent)
    return any(bundle == root or bundle.is_relative_to(root) for root in roots)


def expected_asset_name(system: str | None = None, machine: str | None = None, *, onedir: bool | None = None) -> str:
    use_onedir = running_onedir() if onedir is None else onedir
    suffix = "-onedir" if use_onedir else ""
    return f"SSHMountMate-{current_platform_name(system)}-{current_arch(machine)}{suffix}.zip"


def release_assets(raw_assets: list[dict[str, Any]]) -> list[ReleaseAsset]:
    assets: list[ReleaseAsset] = []
    for item in raw_assets:
        name = str(item.get("name") or "")
        url = str(item.get("browser_download_url") or "")
        if name and url:
            try:
                size = int(item.get("size") or 0)
            except (TypeError, ValueError):
                size = 0
            assets.append(ReleaseAsset(name=name, url=url, digest=str(item.get("digest") or ""), size=max(0, size)))
    return assets


def select_asset(assets: list[ReleaseAsset], expected_name: str | None = None) -> ReleaseAsset | None:
    expected = expected_name or expected_asset_name()
    for asset in assets:
        if asset.name == expected:
            return asset
    expected_key = expected.removesuffix(".zip").casefold()
    for asset in assets:
        if expected_key in asset.name.casefold():
            return asset
    return None


def fetch_latest_release(timeout: float = 8.0) -> dict[str, Any]:
    request = urllib.request.Request(
        LATEST_RELEASE_API,
        headers={
            "Accept": "application/vnd.github+json",
            "User-Agent": "SSHMountMate-update-check",
            "X-GitHub-Api-Version": "2022-11-28",
        },
    )
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            return json.loads(response.read().decode("utf-8"))
    except urllib.error.HTTPError as exc:
        if exc.code == 403:
            raise RuntimeError(f"GitHub API rate limit or access check failed. Open {LATEST_RELEASE_PAGE} manually.") from exc
        raise RuntimeError(f"GitHub release check failed: HTTP {exc.code}. Open {LATEST_RELEASE_PAGE} manually.") from exc
    except urllib.error.URLError as exc:
        reason = getattr(exc, "reason", exc)
        raise RuntimeError(f"Cannot reach GitHub releases: {reason}. Open {LATEST_RELEASE_PAGE} manually.") from exc


def fetch_latest_release_redirect(timeout: float = 8.0) -> tuple[str, str]:
    request = urllib.request.Request(
        LATEST_RELEASE_PAGE,
        headers={"User-Agent": "SSHMountMate-update-check"},
    )
    with urllib.request.urlopen(request, timeout=timeout) as response:
        final_url = response.geturl()
    path = urllib.parse.urlparse(final_url).path
    match = re.search(r"/releases/tag/([^/?#]+)", path)
    if not match:
        raise RuntimeError(f"GitHub latest release redirect did not include a tag. Open {RELEASES_PAGE} manually.")
    tag = urllib.parse.unquote(match.group(1))
    return tag, final_url


def check_for_updates(current_version: str, timeout: float = 8.0) -> UpdateInfo:
    try:
        data = fetch_latest_release(timeout=timeout)
    except Exception:
        latest, release_url = fetch_latest_release_redirect(timeout=timeout)
        expected = expected_asset_name()
        asset_url = f"https://github.com/{REPO}/releases/download/{urllib.parse.quote(latest, safe='')}/{expected}"
        return UpdateInfo(
            current_version=current_version,
            latest_version=latest,
            release_name=f"SSH MountMate {latest}",
            release_url=release_url,
            is_newer=compare_versions(current_version, latest) < 0,
            asset=ReleaseAsset(name=expected, url=asset_url),
            expected_asset=expected,
            body="",
        )

    latest = str(data.get("tag_name") or "").strip()
    if not latest:
        raise RuntimeError(f"GitHub latest release did not include a tag. Open {RELEASES_PAGE} manually.")
    assets = release_assets(list(data.get("assets") or []))
    expected = expected_asset_name()
    asset = select_asset(assets, expected)
    release_url = str(data.get("html_url") or LATEST_RELEASE_PAGE)
    release_name = str(data.get("name") or latest)
    return UpdateInfo(
        current_version=current_version,
        latest_version=latest,
        release_name=release_name,
        release_url=release_url,
        is_newer=compare_versions(current_version, latest) < 0,
        asset=asset,
        expected_asset=expected,
        body=str(data.get("body") or ""),
    )


def verified_sha256(asset: ReleaseAsset) -> str:
    match = re.fullmatch(r"sha256:([0-9a-fA-F]{64})", asset.digest.strip())
    if not match:
        raise RuntimeError("This release asset does not provide a trusted SHA-256 digest. Open the release page and update manually.")
    return match.group(1).lower()


def validate_asset_url(url: str) -> None:
    parsed = urllib.parse.urlparse(url)
    if parsed.scheme != "https" or parsed.hostname not in {"github.com", "www.github.com"}:
        raise RuntimeError("Automatic updates only accept HTTPS assets hosted on github.com.")


def download_verified_asset(
    asset: ReleaseAsset,
    destination: Path,
    *,
    timeout: float = 30.0,
    progress=None,
) -> Path:
    validate_asset_url(asset.url)
    expected_digest = verified_sha256(asset)
    destination.parent.mkdir(parents=True, exist_ok=True)
    temporary = destination.with_suffix(destination.suffix + ".part")
    temporary.unlink(missing_ok=True)
    request = urllib.request.Request(asset.url, headers={"User-Agent": "SSHMountMate-self-update"})
    digest = hashlib.sha256()
    received = 0
    maximum = max(asset.size, 1) if asset.size else 512 * 1024 * 1024
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response, temporary.open("wb") as output:
            final_host = urllib.parse.urlparse(response.geturl()).hostname
            if final_host not in {"github.com", "www.github.com", "release-assets.githubusercontent.com", "objects.githubusercontent.com"}:
                raise RuntimeError("GitHub redirected the update download to an unexpected host.")
            header_size = int(response.headers.get("Content-Length") or 0)
            total = asset.size or header_size
            while True:
                chunk = response.read(1024 * 1024)
                if not chunk:
                    break
                received += len(chunk)
                if received > maximum + 1024 * 1024:
                    raise RuntimeError("Downloaded update is larger than the published release asset.")
                output.write(chunk)
                digest.update(chunk)
                if progress:
                    progress(received, total)
    except Exception:
        temporary.unlink(missing_ok=True)
        raise
    if asset.size and received != asset.size:
        temporary.unlink(missing_ok=True)
        raise RuntimeError(f"Downloaded update size mismatch: expected {asset.size}, received {received}.")
    actual_digest = digest.hexdigest()
    if actual_digest != expected_digest:
        temporary.unlink(missing_ok=True)
        raise RuntimeError("Downloaded update failed SHA-256 verification.")
    temporary.replace(destination)
    return destination


def safe_extract_zip(archive: Path, destination: Path, *, max_files: int = 20000, max_size: int = 2 * 1024**3) -> Path:
    if destination.exists():
        shutil.rmtree(destination)
    destination.mkdir(parents=True)
    root = destination.resolve()
    total_size = 0
    with zipfile.ZipFile(archive) as bundle:
        members = bundle.infolist()
        if len(members) > max_files:
            raise RuntimeError("Update archive contains too many files.")
        for member in members:
            normalized = member.filename.replace("\\", "/")
            parts = Path(normalized).parts
            if not normalized or normalized.startswith("/") or ".." in parts or re.match(r"^[A-Za-z]:", normalized):
                raise RuntimeError(f"Unsafe path in update archive: {member.filename}")
            mode = member.external_attr >> 16
            if stat.S_ISLNK(mode):
                raise RuntimeError(f"Symbolic links are not allowed in update archives: {member.filename}")
            total_size += max(0, member.file_size)
            if total_size > max_size:
                raise RuntimeError("Update archive expands beyond the safety limit.")
            target = (root / normalized).resolve()
            if not target.is_relative_to(root):
                raise RuntimeError(f"Unsafe path in update archive: {member.filename}")
            if member.is_dir():
                target.mkdir(parents=True, exist_ok=True)
                continue
            target.parent.mkdir(parents=True, exist_ok=True)
            with bundle.open(member) as source, target.open("wb") as output:
                shutil.copyfileobj(source, output)
            permissions = mode & 0o777
            if permissions:
                target.chmod(permissions)
    return destination


def format_update_info(info: UpdateInfo, *, language: str = "en") -> str:
    if language == "zh":
        lines = [
            f"当前版本：{info.current_version}",
            f"最新版本：{info.latest_version}",
            f"Release：{info.release_name}",
            f"页面：{info.release_url}",
            "",
            "状态：" + ("发现新版本。" if info.is_newer else "当前已是最新版本。"),
            f"当前平台匹配包：{info.expected_asset}",
        ]
        if info.asset:
            lines.append(f"下载地址：{info.asset.url}")
        else:
            lines.append("没有在 release 里找到当前平台对应的下载包，请打开页面手动查看。")
        if info.body.strip():
            lines.extend(["", "Release notes:", info.body.strip()])
        return "\n".join(lines)

    lines = [
        f"Current version: {info.current_version}",
        f"Latest version: {info.latest_version}",
        f"Release: {info.release_name}",
        f"Page: {info.release_url}",
        "",
        "Status: " + ("Update available." if info.is_newer else "You are up to date."),
        f"Expected asset: {info.expected_asset}",
    ]
    if info.asset:
        lines.append(f"Download: {info.asset.url}")
    else:
        lines.append("No matching asset was found for this platform. Open the release page manually.")
    if info.body.strip():
        lines.extend(["", "Release notes:", info.body.strip()])
    return "\n".join(lines)
