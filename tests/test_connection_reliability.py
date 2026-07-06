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

    def test_default_mount_command_uses_conservative_cache_settings(self):
        cmd = gui.mount_command(
            {"id": "server", "name": "server"},
            "rclone",
            gui.default_settings(),
            remote="server:",
            mountpoint="/mnt/server",
            cache_dir=Path("/tmp/cache"),
            log_path=Path("/tmp/server.log"),
            rc_addr="127.0.0.1:1234",
        )

        self.assertEqual(cmd[cmd.index("--vfs-cache-mode") + 1], "minimal")
        self.assertEqual(cmd[cmd.index("--vfs-cache-max-size") + 1], "10G")
        self.assertEqual(cmd[cmd.index("--vfs-cache-min-free-space") + 1], "10G")
        self.assertNotIn("--vfs-write-back", cmd)
        self.assertEqual(cmd[cmd.index("--dir-cache-time") + 1], "30s")

    def test_mount_command_only_passes_write_back_for_write_cache_modes(self):
        settings = gui.default_settings() | {"vfs_cache_mode": "minimal", "vfs_write_back": "0s"}
        cmd = gui.mount_command(
            {"id": "server", "name": "server"},
            "rclone",
            settings,
            remote="server:",
            mountpoint="/mnt/server",
            cache_dir=Path("/tmp/cache"),
            log_path=Path("/tmp/server.log"),
            rc_addr="127.0.0.1:1234",
        )
        self.assertNotIn("--vfs-write-back", cmd)

        settings["vfs_cache_mode"] = "full"
        cmd = gui.mount_command(
            {"id": "server", "name": "server"},
            "rclone",
            settings,
            remote="server:",
            mountpoint="/mnt/server",
            cache_dir=Path("/tmp/cache"),
            log_path=Path("/tmp/server.log"),
            rc_addr="127.0.0.1:1234",
        )
        self.assertEqual(cmd[cmd.index("--vfs-write-back") + 1], "0s")

    def test_migrate_legacy_default_cache_settings_to_conservative_defaults(self):
        migrated = gui.migrate_settings(
            gui.default_settings() | {"vfs_cache_mode": "writes", "vfs_cache_max_size": "", "vfs_cache_min_free_space": "", "dir_cache_time": ""},
            {"vfs_cache_mode": "writes", "vfs_cache_max_size": "", "vfs_cache_min_free_space": "", "dir_cache_time": ""},
        )

        self.assertEqual(migrated["vfs_cache_mode"], "minimal")
        self.assertEqual(migrated["vfs_cache_max_size"], "10G")
        self.assertEqual(migrated["vfs_cache_min_free_space"], "10G")
        self.assertEqual(migrated["vfs_write_back"], "0s")
        self.assertEqual(migrated["dir_cache_time"], "30s")
        self.assertEqual(migrated["settings_schema_version"], gui.SETTINGS_SCHEMA_VERSION)

    def test_migrate_preserves_custom_write_cache_settings(self):
        migrated = gui.migrate_settings(
            gui.default_settings() | {"vfs_cache_mode": "writes", "vfs_cache_max_size": "50G", "vfs_cache_min_free_space": "5G", "dir_cache_time": "1m"},
            {"vfs_cache_mode": "writes", "vfs_cache_max_size": "50G", "vfs_cache_min_free_space": "5G", "dir_cache_time": "1m"},
        )

        self.assertEqual(migrated["vfs_cache_mode"], "writes")
        self.assertEqual(migrated["vfs_cache_max_size"], "50G")
        self.assertEqual(migrated["vfs_cache_min_free_space"], "5G")
        self.assertEqual(migrated["dir_cache_time"], "1m")

    def test_migrate_local_v2_off_default_to_minimal(self):
        migrated = gui.migrate_settings(
            gui.default_settings() | {"vfs_cache_mode": "off", "vfs_write_back": ""},
            {"settings_schema_version": 2, "vfs_cache_mode": "off", "vfs_write_back": ""},
        )

        self.assertEqual(migrated["vfs_cache_mode"], "minimal")
        self.assertEqual(migrated["vfs_write_back"], "0s")

    def test_refresh_mounted_directory_caches_only_for_mounted_servers(self):
        servers = [{"id": "mounted", "name": "mounted"}, {"id": "stopped", "name": "stopped"}]
        refreshed: list[str] = []

        def fake_status(server, processes):
            return "mounted" if server["id"] == "mounted" else "stopped"

        with mock.patch.object(gui, "running_rclone_processes", return_value={}):
            with mock.patch.object(gui, "mount_status_with_processes", side_effect=fake_status):
                with mock.patch.object(gui, "refresh_remote_cache", side_effect=lambda server, rclone: refreshed.append(server["id"])):
                    errors = gui.refresh_mounted_directory_caches(servers, "rclone")

        self.assertEqual(errors, [])
        self.assertEqual(refreshed, ["mounted"])

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

    def test_write_managed_ssh_config_uses_clear_app_header(self):
        with tempfile.TemporaryDirectory() as temp_name:
            ssh_dir = Path(temp_name) / ".ssh"
            key_file = Path(temp_name) / "id_ed25519"
            key_file.write_text("PRIVATE KEY", encoding="utf-8")
            server = {
                "host_alias": "SAI-user",
                "host": "c1.example",
                "user": "user",
                "port": "12022",
                "key_file": str(key_file),
            }

            original_user_ssh_dir = gui.user_ssh_dir
            original_restrict = gui.windows_restrict_ssh_permissions
            try:
                gui.user_ssh_dir = lambda: ssh_dir
                gui.windows_restrict_ssh_permissions = lambda _path: None
                path = gui.write_managed_ssh_config(server)
            finally:
                gui.user_ssh_dir = original_user_ssh_dir
                gui.windows_restrict_ssh_permissions = original_restrict

            content = path.read_text(encoding="utf-8").splitlines()
            self.assertEqual(content[0], "# Managed by SSH MountMate.")
            self.assertEqual(content[1], "# Prefer editing this Host from the SSH MountMate app.")
            self.assertIn("Host SAI-user", content)
            self.assertNotIn("Password", "\n".join(content))

    def test_batch_plan_disables_overwrite_for_protected_match(self):
        with tempfile.TemporaryDirectory() as temp_name:
            config_path = Path(temp_name) / "config"
            config_path.write_text(
                "\n".join(
                    [
                        "Host cluster",
                        "  HostName cluster.example",
                        "  User user",
                        "  Port 22",
                    ]
                ),
                encoding="utf-8",
            )
            existing = {
                "id": "cluster",
                "name": "cluster",
                "host_alias": "cluster",
                "host": "cluster.example",
                "user": "user",
                "port": "22",
            }

            plan = gui.ssh_config_batch_plan(config_path, [existing], protected_ids={"cluster"})

        item = plan["items"][0]
        self.assertEqual(item["status"], "SAME")
        self.assertFalse(item["can_overwrite"])
        self.assertTrue(item["overwrite_protected"])

    def test_batch_plan_allows_overwrite_for_unprotected_same_match(self):
        with tempfile.TemporaryDirectory() as temp_name:
            config_path = Path(temp_name) / "config"
            config_path.write_text(
                "\n".join(
                    [
                        "Host cluster",
                        "  HostName cluster.example",
                        "  User user",
                        "  Port 22",
                    ]
                ),
                encoding="utf-8",
            )
            existing = {
                "id": "cluster",
                "name": "cluster",
                "host_alias": "cluster",
                "host": "cluster.example",
                "user": "user",
                "port": "22",
            }

            plan = gui.ssh_config_batch_plan(config_path, [existing])

        item = plan["items"][0]
        self.assertEqual(item["status"], "SAME")
        self.assertTrue(item["can_overwrite"])
        self.assertFalse(item["overwrite_protected"])

    def test_batch_plan_marks_protected_same_host_without_direct_overwrite(self):
        with tempfile.TemporaryDirectory() as temp_name:
            config_path = Path(temp_name) / "config"
            config_path.write_text(
                "\n".join(
                    [
                        "Host cluster",
                        "  HostName new.example",
                        "  User user",
                        "  Port 22",
                    ]
                ),
                encoding="utf-8",
            )
            existing = {
                "id": "cluster",
                "name": "cluster",
                "host_alias": "cluster",
                "host": "old.example",
                "user": "user",
                "port": "22",
            }

            plan = gui.ssh_config_batch_plan(config_path, [existing], protected_ids={"cluster"})

        item = plan["items"][0]
        self.assertEqual(item["status"], "SAME HOST")
        self.assertFalse(item["can_overwrite"])
        self.assertTrue(item["overwrite_protected"])


if __name__ == "__main__":
    unittest.main()
