from __future__ import annotations

import json
import os
import shutil
import tempfile
import unittest
import zipfile
from pathlib import Path
from unittest import mock

from ssh_mountmate import self_update


class SelfUpdateTests(unittest.TestCase):
    def test_cleanup_update_cache_removes_only_expired_entries(self):
        with tempfile.TemporaryDirectory() as temp_name:
            root = Path(temp_name) / "updates"
            old = root / "old"
            recent = root / "recent"
            old.mkdir(parents=True)
            recent.mkdir()
            (old / "archive.zip").write_bytes(b"old")
            os.utime(old, (0, 0))
            os.utime(recent, (10000, 10000))
            with mock.patch.object(self_update, "user_data_dir", return_value=Path(temp_name)):
                self_update.cleanup_update_cache(max_age_seconds=3600, now=10000)

            self.assertFalse(old.exists())
            self.assertTrue(recent.exists())

    def test_current_install_layout_rejects_temporary_zip_launch(self):
        self.assertTrue(
            self_update.running_from_temporary_dir(
                Path("/tmp/archive/SSHMountMate.exe"),
                temp_dir=Path("/tmp"),
                windows=True,
            )
        )

    def test_current_install_layout_detects_onedir(self):
        executable = "/opt/SSHMountMate/SSHMountMate"
        with mock.patch.object(self_update.sys, "frozen", True, create=True):
            with mock.patch.object(self_update.sys, "executable", executable):
                with mock.patch.object(self_update.sys, "platform", "linux"):
                    with mock.patch.object(self_update, "running_onedir", return_value=True):
                        layout = self_update.current_install_layout()

        self.assertEqual(layout.kind, "directory")
        self.assertEqual(layout.target, Path(executable).resolve().parent)
        self.assertEqual(layout.executable_relative, Path("SSHMountMate"))

    def test_current_install_layout_detects_macos_app(self):
        executable = "/Applications/SSHMountMate.app/Contents/MacOS/SSHMountMate"
        with mock.patch.object(self_update.sys, "frozen", True, create=True):
            with mock.patch.object(self_update.sys, "executable", executable):
                with mock.patch.object(self_update.sys, "platform", "darwin"):
                    layout = self_update.current_install_layout()

        expected_target = next(parent for parent in Path(executable).resolve().parents if parent.suffix.casefold() == ".app")
        self.assertEqual(layout.target, expected_target)
        self.assertEqual(layout.executable_relative, Path("Contents/MacOS/SSHMountMate"))

    def test_find_and_stage_onedir_payload(self):
        with tempfile.TemporaryDirectory() as temp_name:
            root = Path(temp_name)
            extracted = root / "extracted"
            payload = extracted / "SSHMountMate"
            (payload / "_internal").mkdir(parents=True)
            (payload / "SSHMountMate.exe").write_bytes(b"new")
            target = root / "installed" / "SSHMountMate"
            target.mkdir(parents=True)
            layout = self_update.InstallLayout("directory", target, Path("SSHMountMate.exe"))

            found = self_update.find_update_payload(extracted, layout)
            prepared, backup = self_update.stage_payload(found, layout, "v1.2.3")

            self.assertEqual(found, payload)
            self.assertEqual((prepared / "SSHMountMate.exe").read_bytes(), b"new")
            self.assertFalse(backup.exists())

    def test_windows_plan_uses_manifest_without_embedding_paths_in_script(self):
        with tempfile.TemporaryDirectory() as temp_name:
            root = Path(temp_name)
            layout = self_update.InstallLayout("file", root / "SSH MountMate.exe", Path("SSH MountMate.exe"))
            plan = self_update.create_windows_plan(layout, root / ".new.exe", root / ".backup.exe", root / "update")
            manifest = json.loads(plan.manifest.read_text(encoding="utf-8"))

            self.assertEqual(manifest["target"], str(layout.target))
            self.assertIn("Wait-Process", plan.script.read_text(encoding="utf-8-sig"))
            self.assertIn("powershell.exe", plan.command[0])

    def test_existing_backup_blocks_staging(self):
        with tempfile.TemporaryDirectory() as temp_name:
            root = Path(temp_name)
            source = root / "new.exe"
            source.write_bytes(b"new")
            target = root / "SSHMountMate.exe"
            target.write_bytes(b"old")
            backup = target.with_name(f".{target.name}.update-backup")
            backup.write_bytes(b"recovery")
            layout = self_update.InstallLayout("file", target, Path(target.name))

            with self.assertRaisesRegex(RuntimeError, "backup still exists"):
                self_update.stage_payload(source, layout, "v1")

    def test_prepare_update_stages_verified_archive(self):
        with tempfile.TemporaryDirectory() as temp_name:
            root = Path(temp_name)
            archive = root / "source.zip"
            with zipfile.ZipFile(archive, "w") as bundle:
                bundle.writestr("SSHMountMate", b"new executable")
            target = root / "install" / "SSHMountMate"
            target.parent.mkdir()
            target.write_bytes(b"old executable")
            layout = self_update.InstallLayout("file", target, Path(target.name))
            asset = self_update.ReleaseAsset("SSHMountMate-linux-x64.zip", "https://github.com/example/update.zip")

            def fake_download(_asset, destination, **_kwargs):
                shutil.copy2(archive, destination)
                return destination

            with mock.patch.object(self_update, "download_verified_asset", side_effect=fake_download):
                with mock.patch.object(self_update, "current_install_layout", return_value=layout):
                    plan = self_update.prepare_update(asset, "v1.0.0", update_root=root / "work")

            self.assertEqual(plan.prepared.read_bytes(), b"new executable")
            self.assertTrue(plan.script.exists())


if __name__ == "__main__":
    unittest.main()
