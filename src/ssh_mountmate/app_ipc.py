from __future__ import annotations

import json
import secrets
import socket
import threading
from pathlib import Path
from typing import Callable

from .core import write_private_text


def receive_message(connection: socket.socket) -> dict:
    chunks: list[bytes] = []
    total = 0
    while total <= 65536:
        chunk = connection.recv(min(4096, 65537 - total))
        if not chunk:
            break
        chunks.append(chunk)
        total += len(chunk)
        if b"\n" in chunk:
            break
    raw = b"".join(chunks).split(b"\n", 1)[0]
    if not raw or total > 65536:
        raise ValueError("invalid command size")
    message = json.loads(raw.decode("utf-8"))
    if not isinstance(message, dict):
        raise ValueError("invalid command message")
    return message


class AppCommandServer:
    def __init__(self, state_path: Path, callback: Callable[[dict], None]):
        self.state_path = state_path
        self.callback = callback
        self.token = secrets.token_hex(24)
        self.socket = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        self.socket.bind(("127.0.0.1", 0))
        self.socket.listen(4)
        self.socket.settimeout(0.5)
        self.port = int(self.socket.getsockname()[1])
        self.stopped = threading.Event()
        write_private_text(self.state_path, json.dumps({"port": self.port, "token": self.token}))
        self.thread = threading.Thread(target=self._serve, daemon=True)
        self.thread.start()

    def _serve(self) -> None:
        while not self.stopped.is_set():
            try:
                connection, _address = self.socket.accept()
            except socket.timeout:
                continue
            except OSError:
                break
            with connection:
                connection.settimeout(2)
                try:
                    request = receive_message(connection)
                    if not secrets.compare_digest(str(request.get("token") or ""), self.token):
                        raise ValueError("invalid command token")
                    command = request.get("command")
                    if not isinstance(command, dict):
                        raise ValueError("invalid command")
                    self.callback(command)
                    response = {"ok": True}
                except Exception as exc:
                    response = {"ok": False, "error": str(exc)}
                try:
                    connection.sendall(json.dumps(response).encode("utf-8") + b"\n")
                except OSError:
                    pass

    def close(self) -> None:
        self.stopped.set()
        try:
            self.socket.close()
        except OSError:
            pass
        try:
            current = json.loads(self.state_path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError):
            current = {}
        if current.get("token") == self.token:
            self.state_path.unlink(missing_ok=True)


def send_app_command(state_path: Path, command: dict, *, timeout: float = 1.5) -> bool:
    try:
        state = json.loads(state_path.read_text(encoding="utf-8"))
        port = int(state["port"])
        token = str(state["token"])
    except (OSError, ValueError, KeyError, TypeError, json.JSONDecodeError):
        return False
    try:
        with socket.create_connection(("127.0.0.1", port), timeout=timeout) as connection:
            connection.settimeout(timeout)
            connection.sendall(json.dumps({"token": token, "command": command}).encode("utf-8") + b"\n")
            response = receive_message(connection)
    except (OSError, ValueError, json.JSONDecodeError):
        return False
    return bool(isinstance(response, dict) and response.get("ok"))
