from __future__ import annotations

import subprocess
import unittest
from collections import namedtuple

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

    def test_startup_all_command_uses_headless_batch_argument(self):
        command = gui.startup_all_command()

        self.assertIn("--mount-startup-all", command)

    def test_capacity_cache_due_respects_success_and_failure_ttl(self):
        self.assertTrue(gui.capacity_cache_due("server", {}, {}, now=100.0, ttl=60.0))
        self.assertFalse(gui.capacity_cache_due("server", {"server": {"used": 1}}, {"server": 80.0}, now=100.0, ttl=60.0))
        self.assertFalse(gui.capacity_cache_due("server", {}, {"server": 80.0}, now=100.0, ttl=60.0))
        self.assertTrue(gui.capacity_cache_due("server", {"server": {"used": 1}}, {"server": 20.0}, now=100.0, ttl=60.0))

    def test_windows_disk_usage_path_normalizes_drive_root(self):
        self.assertEqual(gui.disk_usage_path("z:", windows=True), "Z:\\")
        self.assertEqual(gui.disk_usage_path("Z:\\", windows=True), "Z:\\")

    def test_capacity_info_prefers_local_mountpoint_without_remote_probe(self):
        usage = namedtuple("usage", "total used free")(1000, 250, 750)
        server = {"id": "server", "mountpoint": "/mnt/server"}

        original_current_state = gui.current_state
        original_current_mountpoint = gui.current_mountpoint
        original_disk_usage = gui.shutil.disk_usage
        original_remote_capacity_info = gui.remote_capacity_info
        try:
            gui.current_state = lambda _server: {"mountpoint": "/mnt/server"}
            gui.current_mountpoint = lambda _server: "/mnt/server"
            gui.shutil.disk_usage = lambda _path: usage

            def fail_remote(*_args, **_kwargs):
                raise AssertionError("remote capacity should not be queried when local capacity is available")

            gui.remote_capacity_info = fail_remote
            capacity = gui.capacity_info(server, "rclone", "mounted")
        finally:
            gui.current_state = original_current_state
            gui.current_mountpoint = original_current_mountpoint
            gui.shutil.disk_usage = original_disk_usage
            gui.remote_capacity_info = original_remote_capacity_info

        self.assertEqual(capacity["source"], "local_mountpoint")
        self.assertEqual(capacity["used"], 250)
        self.assertEqual(capacity["total"], 1000)
        self.assertEqual(capacity["percent"], 25)

    def test_local_capacity_cache_due_uses_short_ttl(self):
        self.assertFalse(gui.local_capacity_cache_due("server", {"server": 100.0}, now=104.0))
        self.assertTrue(gui.local_capacity_cache_due("server", {"server": 100.0}, now=106.0))

    def test_display_remote_path_expands_home_paths(self):
        self.assertEqual(gui.display_remote_path(""), "$HOME")
        self.assertEqual(gui.display_remote_path("project/data"), "$HOME/project/data")
        self.assertEqual(gui.display_remote_path("/scratch/project"), "/scratch/project")

    def test_shorten_middle_text_keeps_both_ends(self):
        self.assertEqual(gui.shorten_middle_text("/very/long/path/name", 12), "/ver.../name")

    def test_local_mount_display_path_uses_current_mountpoint(self):
        server = {"id": "server", "mountpoint": "Z:", "remote_path": "project/data"}

        original_current_state = gui.current_state
        try:
            gui.current_state = lambda _server: {"mountpoint": "Z:"}
            self.assertEqual(
                gui.local_mount_display_path(server, "mounted"),
                gui.disk_usage_path("Z:"),
            )
        finally:
            gui.current_state = original_current_state

    def test_simple_mount_status_accepts_running_pid_without_command_line(self):
        state = {"pid": 42, "remote": "host:/data", "mountpoint": "Z:"}

        self.assertEqual(gui.simple_mount_status_from_state(state, processes={42: ""}), "mounted")

    def test_simple_mount_status_uses_ready_mountpoint_as_mounted(self):
        state = {"pid": 42, "remote": "host:/data", "mountpoint": "Z:"}
        original_mountpoint_ready = gui.mountpoint_ready
        try:
            gui.mountpoint_ready = lambda _mountpoint: True
            status = gui.simple_mount_status_from_state(state, processes={})
        finally:
            gui.mountpoint_ready = original_mountpoint_ready

        self.assertEqual(status, "mounted")


if __name__ == "__main__":
    unittest.main()
