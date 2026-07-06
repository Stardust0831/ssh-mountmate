from __future__ import annotations

import configparser
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from ssh_mountmate import core, gui


class ConnectionReliabilityTests(unittest.TestCase):
    def test_scan_host_keys_normalizes_to_rclone_marker(self):
        output = "\n".join(
            [
                "# c1.example:12022 SSH-2.0",
                "[c1.example]:12022 ssh-ed25519 AAAATEST1",
                "c1.example ssh-rsa AAAATEST2",
            ]
        )

        with mock.patch.object(core.shutil, "which", return_value="ssh-keyscan"):
            with mock.patch.object(core.subprocess, "run") as run_mock:
                run_mock.return_value.stdout = output
                lines = core.scan_host_keys("c1.example", "12022")

        self.assertEqual(
            lines,
            [
                "[c1.example]:12022 ssh-ed25519 AAAATEST1",
                "[c1.example]:12022 ssh-rsa AAAATEST2",
            ],
        )

    def test_write_manual_remote_can_remove_known_hosts_file(self):
        with tempfile.TemporaryDirectory() as temp_name:
            conf_path = Path(temp_name) / "rclone.conf"
            known_hosts = Path(temp_name) / "known_hosts"
            known_hosts.write_text("host ssh-ed25519 AAAA\n", encoding="utf-8")
            server = {"id": "server", "host": "host", "user": "user", "port": "22", "auth": "key", "key_file": "id_ed25519"}

            original_path = core.rclone_config_path
            try:
                core.rclone_config_path = lambda: conf_path
                gui.write_manual_remote_unlocked(server, "rclone", known_hosts)
                gui.write_manual_remote_unlocked(server, "rclone", None)
            finally:
                core.rclone_config_path = original_path

            parser = configparser.RawConfigParser()
            parser.read(conf_path)
            self.assertFalse(parser.has_option("server", "known_hosts_file"))

    def test_mount_command_enables_links(self):
        cmd = gui.mount_command(
            {"id": "server", "name": "server", "cache_mode": "writes"},
            "rclone",
            {},
            remote="server:",
            mountpoint="/mnt/server",
            cache_dir=Path("/tmp/cache"),
            log_path=Path("/tmp/server.log"),
            rc_addr="127.0.0.1:1234",
        )

        self.assertIn("--links", cmd)

    def test_log_has_known_hosts_mismatch(self):
        with tempfile.TemporaryDirectory() as temp_name:
            log_path = Path(temp_name) / "mount.log"
            log_path.write_text("ssh: handshake failed: knownhosts: key mismatch\n", encoding="utf-8")

            self.assertTrue(gui.log_has_known_hosts_mismatch(log_path))

    def test_copy_key_to_user_ssh_restricts_copied_key(self):
        with tempfile.TemporaryDirectory() as temp_name:
            root = Path(temp_name)
            source = root / "id_ed25519"
            source.write_text("PRIVATE KEY", encoding="utf-8")
            ssh_dir = root / ".ssh"
            restricted: list[Path] = []

            original_user_ssh_dir = gui.user_ssh_dir
            original_restrict = gui.windows_restrict_ssh_permissions
            try:
                gui.user_ssh_dir = lambda: ssh_dir
                gui.windows_restrict_ssh_permissions = lambda path: restricted.append(Path(path))
                copied = Path(gui.copy_key_to_user_ssh(str(source), "SAI-user"))
            finally:
                gui.user_ssh_dir = original_user_ssh_dir
                gui.windows_restrict_ssh_permissions = original_restrict

            self.assertEqual(copied, ssh_dir / "SAI-user")
            self.assertEqual(copied.read_text(encoding="utf-8"), "PRIVATE KEY")
            self.assertIn(copied, restricted)


if __name__ == "__main__":
    unittest.main()
