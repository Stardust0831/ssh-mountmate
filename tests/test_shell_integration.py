from __future__ import annotations

import unittest
from unittest import mock

from ssh_mountmate import shell_integration


class ShellIntegrationTests(unittest.TestCase):
    def test_windows_context_command_quotes_explorer_path_placeholder(self):
        with mock.patch.object(shell_integration, "application_command", return_value=[r"C:\Program Files\SSH MountMate\SSHMountMate.exe"]):
            command = shell_integration.windows_command_line("--refresh-path", "%V")

        self.assertIn('"C:\\Program Files\\SSH MountMate\\SSHMountMate.exe"', command)
        self.assertTrue(command.endswith('"%V\\."'))

    def test_transfer_command_does_not_add_empty_argument(self):
        with mock.patch.object(shell_integration, "application_command", return_value=["SSHMountMate.exe"]):
            command = shell_integration.windows_command_line("--show-transfers", "")

        self.assertEqual(command, "SSHMountMate.exe --show-transfers")


if __name__ == "__main__":
    unittest.main()
