from __future__ import annotations

import tempfile
import unittest
from pathlib import Path
from unittest import mock

from ssh_mountmate import rclone


class RcloneResolutionTests(unittest.TestCase):
    def test_existing_materialized_bundled_rclone_skips_process_cleanup(self):
        with tempfile.TemporaryDirectory() as temp_name:
            root = Path(temp_name)
            source = root / "rclone"
            source.write_bytes(b"fake-rclone")
            target = root / "rclone-existing"
            target.write_bytes(b"fake-rclone")
            with mock.patch.object(rclone, "materialized_bundled_rclone_path", return_value=target):
                with mock.patch.object(rclone, "cleanup_managed_bundled_rclones") as cleanup:
                    self.assertEqual(rclone.materialize_bundled_rclone(source), target)

            cleanup.assert_not_called()

    def test_resolve_rclone_materializes_bundled_binary(self):
        with tempfile.TemporaryDirectory() as temp_name:
            root = Path(temp_name)
            app_root = root / "app"
            bundled_dir = app_root / "bin"
            bundled_dir.mkdir(parents=True)
            bundled = bundled_dir / rclone.current_platform().rclone_binary
            bundled.write_bytes(b"fake-rclone")

            original_managed_bin_dir = rclone.managed_bin_dir
            original_legacy_managed_bin_dirs = rclone.legacy_managed_bin_dirs
            try:
                rclone.managed_bin_dir = lambda: root / "managed" / "bin"
                rclone.legacy_managed_bin_dirs = lambda: []
                resolved = Path(rclone.resolve_rclone(app_root))
            finally:
                rclone.managed_bin_dir = original_managed_bin_dir
                rclone.legacy_managed_bin_dirs = original_legacy_managed_bin_dirs

            self.assertNotEqual(resolved, bundled)
            self.assertTrue(resolved.exists())
            self.assertTrue(resolved.name.startswith("rclone-"))
            self.assertEqual(resolved.read_bytes(), b"fake-rclone")

    def test_resolve_rclone_accepts_materialized_managed_binary_without_plain_rclone(self):
        with tempfile.TemporaryDirectory() as temp_name:
            root = Path(temp_name)
            managed_dir = root / "managed" / "bin"
            managed_dir.mkdir(parents=True)
            managed = managed_dir / ("rclone-3ac0dba3a883555f.exe" if rclone.current_platform().system == "Windows" else "rclone-3ac0dba3a883555f")
            managed.write_bytes(b"fake-rclone")

            original_managed_bin_dir = rclone.managed_bin_dir
            original_legacy_managed_bin_dirs = rclone.legacy_managed_bin_dirs
            original_bundled_rclone_candidates = rclone.bundled_rclone_candidates
            try:
                rclone.managed_bin_dir = lambda: managed_dir
                rclone.legacy_managed_bin_dirs = lambda: []
                rclone.bundled_rclone_candidates = lambda _app_root: []
                resolved = Path(rclone.resolve_rclone(root / "app"))
            finally:
                rclone.managed_bin_dir = original_managed_bin_dir
                rclone.legacy_managed_bin_dirs = original_legacy_managed_bin_dirs
                rclone.bundled_rclone_candidates = original_bundled_rclone_candidates

            self.assertEqual(resolved, managed)


if __name__ == "__main__":
    unittest.main()
