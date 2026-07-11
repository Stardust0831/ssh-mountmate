from __future__ import annotations

import json
import unittest
from unittest import mock

from ssh_mountmate import rclone_processes


class RcloneProcessesTests(unittest.TestCase):
    def test_windows_rclone_process_name_accepts_managed_and_plain_rclone(self):
        self.assertTrue(rclone_processes.is_windows_rclone_process_name("rclone.exe"))
        self.assertTrue(rclone_processes.is_windows_rclone_process_name("rclone-3ac0dba3a883555f.exe"))
        self.assertTrue(rclone_processes.is_windows_rclone_process_name("RCLONE-3AC0DBA3A883555F.EXE"))
        self.assertFalse(rclone_processes.is_windows_rclone_process_name("rclone-helper.exe"))
        self.assertFalse(rclone_processes.is_windows_rclone_process_name("not-rclone.exe"))

    def test_parse_windows_rclone_processes_keeps_managed_rclone_processes(self):
        output = json.dumps(
            [
                {
                    "ProcessId": 101,
                    "Name": "rclone-3ac0dba3a883555f.exe",
                    "CommandLine": '"C:\\Users\\me\\AppData\\Local\\SSHMountMate\\bin\\rclone-3ac0dba3a883555f.exe" mount host:/data Z:',
                },
                {
                    "ProcessId": 102,
                    "Name": "rclone.exe",
                    "CommandLine": '"C:\\Program Files\\rclone\\rclone.exe" mount host:/data Y:',
                },
                {
                    "ProcessId": 103,
                    "Name": "rclone-helper.exe",
                    "CommandLine": "rclone-helper.exe mount host:/data X:",
                },
            ]
        )

        processes = rclone_processes.parse_windows_rclone_processes(output)

        self.assertEqual(
            processes,
            {
                101: '"C:\\Users\\me\\AppData\\Local\\SSHMountMate\\bin\\rclone-3ac0dba3a883555f.exe" mount host:/data Z:',
                102: '"C:\\Program Files\\rclone\\rclone.exe" mount host:/data Y:',
            },
        )

    def test_parse_windows_tasklist_process_ids_filters_rclone_images(self):
        output = "\n".join(
            [
                '"rclone.exe","1,234","Console","1","12,000 K"',
                '"rclone-3ac0dba3a883555f.exe","5678","Console","1","20,000 K"',
                '"python.exe","9999","Console","1","30,000 K"',
            ]
        )

        self.assertEqual(rclone_processes.parse_windows_tasklist_process_ids(output), {1234, 5678})

    def test_windows_process_id_scan_prefers_native_api(self):
        with mock.patch.object(rclone_processes, "running_windows_rclone_process_ids_native", return_value={42}):
            with mock.patch.object(rclone_processes.subprocess, "run") as run:
                self.assertEqual(rclone_processes.running_windows_rclone_process_ids(), {42})

        run.assert_not_called()


if __name__ == "__main__":
    unittest.main()
