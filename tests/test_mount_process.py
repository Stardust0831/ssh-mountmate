from __future__ import annotations

import tempfile
import unittest
from pathlib import Path
from unittest import mock

from ssh_mountmate import mount_process


class FakeProcess:
    def __init__(self, pid: int = 1234, returncode=None):
        self.pid = pid
        self.returncode = returncode
        self.terminated = False
        self.killed = False

    def poll(self):
        return self.returncode

    def terminate(self):
        self.terminated = True

    def kill(self):
        self.killed = True


class MountProcessTests(unittest.TestCase):
    def test_command_matches_mount_does_not_require_log_path(self):
        state = {
            "remote": "SAI-user:/project",
            "mountpoint": "/Users/me/mnt/SAI-user",
            "log": "/old/path/mount.log",
        }
        command = "rclone mount SAI-user:/project /Users/me/mnt/SAI-user --log-file /new/path/mount.log"

        self.assertFalse(mount_process.command_matches_state(command, state))
        self.assertTrue(mount_process.command_matches_mount(command, state))

    def test_mount_status_rejects_pid_with_wrong_command(self):
        state = {
            "pid": 42,
            "remote": "host:/data",
            "mountpoint": "/mnt/data",
            "log": "/tmp/host.log",
        }
        processes = {42: "python unrelated.py"}

        status = mount_process.mount_status_from_state(
            state,
            processes=processes,
            mountpoint_ready=lambda _value: True,
            allow_pid_fallback=False,
        )

        self.assertEqual(status, "stale")

    def test_mount_status_accepts_pid_fallback_when_command_line_is_unavailable(self):
        state = {
            "pid": 42,
            "remote": "host:/data",
            "mountpoint": "/mnt/data",
            "log": "/tmp/host.log",
        }
        processes = {42: ""}

        status = mount_process.mount_status_from_state(
            state,
            processes=processes,
            mountpoint_ready=lambda _value: True,
            allow_pid_fallback=True,
        )

        self.assertEqual(status, "mounted")

    def test_mount_status_accepts_matching_command_and_ready_mountpoint(self):
        state = {
            "pid": 42,
            "remote": "host:/data",
            "mountpoint": "/mnt/data",
            "log": "/tmp/host.log",
        }
        processes = {42: "rclone mount host:/data /mnt/data --log-file /tmp/other.log"}

        status = mount_process.mount_status_from_state(
            state,
            processes=processes,
            mountpoint_ready=lambda _value: True,
            allow_pid_fallback=False,
        )

        self.assertEqual(status, "mounted")

    def test_running_procfs_rclone_processes_reads_cmdline(self):
        with tempfile.TemporaryDirectory() as temp_name:
            proc = Path(temp_name)
            process = proc / "123"
            process.mkdir()
            (process / "cmdline").write_bytes(b"rclone\x00mount\x00host:/data\x00/mnt/data\x00")
            other = proc / "456"
            other.mkdir()
            (other / "cmdline").write_bytes(b"python\x00script.py\x00")

            processes = mount_process.running_procfs_rclone_processes(proc)

        self.assertEqual(processes, {123: "rclone mount host:/data /mnt/data"})

    def test_wait_for_mount_ready_returns_after_stable_ready_match(self):
        with tempfile.TemporaryDirectory() as temp_name:
            log_path = Path(temp_name) / "mount.log"
            log_path.write_text("", encoding="utf-8")
            state = {"remote": "host:/data", "mountpoint": "/mnt/data", "log": str(log_path)}
            process = FakeProcess()

            mount_process.wait_for_mount_ready(
                process,
                "/mnt/data",
                log_path,
                state,
                ready_before_start=False,
                mountpoint_ready=lambda _value: True,
                stable_for=0,
                process_command_func=lambda _pid: "rclone mount host:/data /mnt/data",
                sleep_func=lambda _seconds: None,
            )

            self.assertFalse(process.terminated)
            self.assertFalse(process.killed)

    def test_wait_for_mount_ready_terminates_when_not_ready(self):
        with tempfile.TemporaryDirectory() as temp_name:
            log_path = Path(temp_name) / "mount.log"
            log_path.write_text("failed\n", encoding="utf-8")
            state = {"remote": "host:/data", "mountpoint": "/mnt/data", "log": str(log_path)}
            process = FakeProcess()

            with self.assertRaises(RuntimeError):
                mount_process.wait_for_mount_ready(
                    process,
                    "/mnt/data",
                    log_path,
                    state,
                    ready_before_start=False,
                    mountpoint_ready=lambda _value: False,
                    timeout=0,
                    process_command_func=lambda _pid: "",
                    sleep_func=lambda _seconds: None,
                )

            self.assertTrue(process.terminated)
            self.assertTrue(process.killed)

    def test_process_matches_state_for_kill_requires_matching_command(self):
        state = {"remote": "host:/data", "mountpoint": "/mnt/data", "log": "/tmp/host.log"}
        with mock.patch.object(mount_process, "process_command", return_value="rclone mount other:/data /mnt/other"):
            self.assertFalse(mount_process.process_matches_state_for_kill(42, state))
        with mock.patch.object(mount_process, "process_command", return_value="rclone mount host:/data /mnt/data"):
            self.assertTrue(mount_process.process_matches_state_for_kill(42, state))


if __name__ == "__main__":
    unittest.main()
