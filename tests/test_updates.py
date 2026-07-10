from __future__ import annotations

import unittest
from unittest import mock

from ssh_mountmate import updates


class UpdateTests(unittest.TestCase):
    def test_running_onedir_detects_internal_directory(self):
        with mock.patch.object(updates.sys, "frozen", True, create=True):
            with mock.patch.object(updates.sys, "_MEIPASS", "/opt/SSHMountMate/_internal", create=True):
                with mock.patch.object(updates.sys, "executable", "/opt/SSHMountMate/SSHMountMate"):
                    self.assertTrue(updates.running_onedir())

    def test_running_onedir_detects_macos_app_frameworks(self):
        with mock.patch.object(updates.sys, "frozen", True, create=True):
            with mock.patch.object(updates.sys, "_MEIPASS", "/Applications/SSHMountMate.app/Contents/Frameworks", create=True):
                with mock.patch.object(updates.sys, "executable", "/Applications/SSHMountMate.app/Contents/MacOS/SSHMountMate"):
                    self.assertTrue(updates.running_onedir())

    def test_expected_asset_preserves_onedir_variant(self):
        self.assertEqual(
            updates.expected_asset_name("Windows", "AMD64", onedir=True),
            "SSHMountMate-windows-x64-onedir.zip",
        )

    def test_expected_asset_keeps_legacy_name_for_onefile(self):
        self.assertEqual(
            updates.expected_asset_name("Linux", "aarch64", onedir=False),
            "SSHMountMate-linux-arm64.zip",
        )


if __name__ == "__main__":
    unittest.main()
