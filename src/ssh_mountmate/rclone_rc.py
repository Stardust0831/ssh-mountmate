from __future__ import annotations

import json
import urllib.error
import urllib.parse
import urllib.request
from typing import Any


def rc_url(rc_addr: str, method: str) -> str:
    address = str(rc_addr or "").strip().removeprefix("http://").removeprefix("https://").rstrip("/")
    if not address:
        raise RuntimeError("This mount does not have a remote-control address.")
    try:
        parsed = urllib.parse.urlsplit("//" + address)
        port = parsed.port
    except ValueError as exc:
        raise RuntimeError(f"Invalid rclone RC address: {rc_addr}") from exc
    if parsed.hostname not in {"127.0.0.1", "localhost", "::1"} or not port or parsed.path:
        raise RuntimeError(f"Refusing non-loopback rclone RC address: {rc_addr}")
    return f"http://{address}/{method.lstrip('/')}"


def rc_call(rc_addr: str, method: str, params: dict[str, Any] | None = None, *, timeout: float = 3.0) -> dict:
    request = urllib.request.Request(
        rc_url(rc_addr, method),
        data=json.dumps(params or {}).encode("utf-8"),
        headers={"Content-Type": "application/json", "User-Agent": "SSHMountMate-rc"},
        method="POST",
    )
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            payload = json.loads(response.read().decode("utf-8") or "{}")
    except urllib.error.HTTPError as exc:
        try:
            detail = exc.read().decode("utf-8", errors="replace")
        except OSError:
            detail = ""
        raise RuntimeError(f"rclone RC {method} failed: HTTP {exc.code} {detail}".strip()) from exc
    except (urllib.error.URLError, TimeoutError, OSError) as exc:
        raise RuntimeError(f"Cannot reach rclone RC {rc_addr}: {exc}") from exc
    except (json.JSONDecodeError, UnicodeDecodeError) as exc:
        raise RuntimeError(f"rclone RC {method} returned invalid JSON") from exc
    if not isinstance(payload, dict):
        raise RuntimeError(f"rclone RC {method} returned an unexpected response")
    if payload.get("error"):
        raise RuntimeError(str(payload["error"]))
    return payload


def normalized_transfer_name(value: object) -> str:
    return str(value or "").replace("\\", "/").strip("/").casefold()


def transfer_matches(queue_name: object, transfer_name: object) -> bool:
    queued = normalized_transfer_name(queue_name)
    active = normalized_transfer_name(transfer_name)
    return bool(queued and active and (queued == active or queued.endswith(f"/{active}") or active.endswith(f"/{queued}")))


def build_transfer_snapshot(queue_result: dict, vfs_result: dict, core_result: dict) -> dict:
    queue = [dict(item) for item in queue_result.get("queue", []) if isinstance(item, dict)]
    transferring = [dict(item) for item in core_result.get("transferring", []) if isinstance(item, dict)]
    files: list[dict] = []
    queued_bytes = 0
    transferred_bytes = 0
    for item in queue:
        size = max(0, int(item.get("size") or 0))
        queued_bytes += size
        active = next((candidate for candidate in transferring if transfer_matches(item.get("name"), candidate.get("name"))), None)
        uploaded = max(0, int((active or {}).get("bytes") or 0))
        uploaded = min(uploaded, size) if size else uploaded
        transferred_bytes += uploaded
        percent = float((active or {}).get("percentage") or (uploaded * 100 / size if size else 0))
        files.append(
            {
                "id": item.get("id"),
                "name": str(item.get("name") or ""),
                "size": size,
                "bytes": uploaded,
                "percentage": max(0.0, min(100.0, percent)),
                "speed": float((active or {}).get("speedAvg") or (active or {}).get("speed") or 0),
                "eta": (active or {}).get("eta"),
                "uploading": bool(item.get("uploading") or active),
                "tries": int(item.get("tries") or 0),
            }
        )
    disk_cache = vfs_result.get("diskCache") if isinstance(vfs_result.get("diskCache"), dict) else {}
    uploads_queued = max(len(queue), int(disk_cache.get("uploadsQueued") or 0))
    uploads_in_progress = max(sum(1 for item in files if item["uploading"]), int(disk_cache.get("uploadsInProgress") or 0))
    errors = int(disk_cache.get("erroredFiles") or 0)
    total = queued_bytes
    percentage = transferred_bytes * 100 / total if total else 100.0
    return {
        "files": files,
        "queued": uploads_queued,
        "uploading": uploads_in_progress,
        "queued_bytes": queued_bytes,
        "transferred_bytes": transferred_bytes,
        "percentage": max(0.0, min(100.0, percentage)),
        "errors": errors,
        "out_of_space": bool(disk_cache.get("outOfSpace")),
        "synced": uploads_queued == 0 and uploads_in_progress == 0 and errors == 0,
    }


def transfer_snapshot(rc_addr: str) -> dict:
    queue = rc_call(rc_addr, "vfs/queue")
    vfs = rc_call(rc_addr, "vfs/stats")
    core = rc_call(rc_addr, "core/stats")
    return build_transfer_snapshot(queue, vfs, core)


def refresh_remote_snapshot(rc_addr: str, remote: str, relative_dir: str = "", *, recursive: bool = False) -> dict:
    queue = rc_call(rc_addr, "vfs/queue")
    queue_items = [item for item in queue.get("queue", []) if isinstance(item, dict)]
    path_params = {"dir": relative_dir} if relative_dir else {}
    rc_call(rc_addr, "vfs/forget", path_params)
    refresh_params: dict[str, Any] = dict(path_params)
    if recursive:
        refresh_params["recursive"] = True
    rc_call(rc_addr, "vfs/refresh", refresh_params)
    listing = rc_call(rc_addr, "operations/list", {"fs": remote, "remote": relative_dir})
    entries = listing.get("list", [])
    return {
        "pending_uploads": len(queue_items),
        "entries": entries if isinstance(entries, list) else [],
    }
