from __future__ import annotations

import subprocess
import unittest

from ssh_mountmate import gui


class StartupErrorTests(unittest.TestCase):
    def test_process_error_details_includes_schtasks_output(self):
        exc = subprocess.CalledProcessError(
            1,
            ["schtasks", "/Create", "/TN", "SSHMountMate-NAS"],
            output="OUT text",
            stderr="ERR text",
        )

        details = gui.process_error_details(exc)

        self.assertIn("schtasks", details)
        self.assertIn("Exit code: 1", details)
        self.assertIn("OUT text", details)
        self.assertIn("ERR text", details)

    def test_command_display_quotes_windows_command_lines(self):
        command = ["schtasks", "/TR", '"G:\\Downloads\\SSHMountMate.exe" --mount-id "NAS"']

        display = gui.command_display(command)

        self.assertIn("schtasks", display)
        self.assertIn("SSHMountMate.exe", display)


if __name__ == "__main__":
    unittest.main()
