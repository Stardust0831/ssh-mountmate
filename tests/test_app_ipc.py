from __future__ import annotations

import tempfile
import threading
import unittest
from pathlib import Path

from ssh_mountmate.app_ipc import AppCommandServer, send_app_command


class AppIpcTests(unittest.TestCase):
    def test_command_server_forwards_authenticated_local_command(self):
        with tempfile.TemporaryDirectory() as temp_name:
            received: list[dict] = []
            event = threading.Event()

            def callback(command: dict) -> None:
                received.append(command)
                event.set()

            server = AppCommandServer(Path(temp_name) / "command.json", callback)
            try:
                self.assertTrue(send_app_command(server.state_path, {"action": "show_transfers"}))
                self.assertTrue(event.wait(2))
            finally:
                server.close()

            self.assertEqual(received, [{"action": "show_transfers"}])
            self.assertFalse(server.state_path.exists())

    def test_missing_command_state_returns_false(self):
        with tempfile.TemporaryDirectory() as temp_name:
            self.assertFalse(send_app_command(Path(temp_name) / "missing.json", {"action": "show_main"}))


if __name__ == "__main__":
    unittest.main()
