from __future__ import annotations

import tempfile
import unittest
from pathlib import Path

from ssh_mountmate import rclone


class RcloneResolutionTests(unittest.TestCase):
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


if __name__ == "__main__":
    unittest.main()
