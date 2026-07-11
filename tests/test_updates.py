from __future__ import annotations

import hashlib
import io
import tempfile
import unittest
import zipfile
from pathlib import Path
from unittest import mock

from ssh_mountmate import updates


class UpdateTests(unittest.TestCase):
    def test_release_assets_keep_published_digest_and_size(self):
        assets = updates.release_assets(
            [{"name": "app.zip", "browser_download_url": "https://github.com/repo/app.zip", "digest": "sha256:" + "a" * 64, "size": 123}]
        )

        self.assertEqual(assets[0].digest, "sha256:" + "a" * 64)
        self.assertEqual(assets[0].size, 123)

    def test_download_verified_asset_checks_sha256(self):
        payload = b"verified update"
        asset = updates.ReleaseAsset(
            name="app.zip",
            url="https://github.com/example/app.zip",
            digest="sha256:" + hashlib.sha256(payload).hexdigest(),
            size=len(payload),
        )

        class Response(io.BytesIO):
            headers = {"Content-Length": str(len(payload))}

            def geturl(self):
                return "https://release-assets.githubusercontent.com/example/app.zip"

            def __enter__(self):
                return self

            def __exit__(self, *_args):
                self.close()

        with tempfile.TemporaryDirectory() as temp_name:
            destination = Path(temp_name) / "app.zip"
            with mock.patch.object(updates.urllib.request, "urlopen", return_value=Response(payload)):
                updates.download_verified_asset(asset, destination)
            self.assertEqual(destination.read_bytes(), payload)

    def test_safe_extract_zip_rejects_parent_traversal(self):
        with tempfile.TemporaryDirectory() as temp_name:
            root = Path(temp_name)
            archive = root / "bad.zip"
            with zipfile.ZipFile(archive, "w") as bundle:
                bundle.writestr("../outside.txt", "bad")
            with self.assertRaisesRegex(RuntimeError, "Unsafe path"):
                updates.safe_extract_zip(archive, root / "out")
            self.assertFalse((root / "outside.txt").exists())

    def test_safe_extract_zip_rejects_symbolic_links(self):
        with tempfile.TemporaryDirectory() as temp_name:
            root = Path(temp_name)
            archive = root / "link.zip"
            link = zipfile.ZipInfo("SSHMountMate/link")
            link.create_system = 3
            link.external_attr = (0o120777 << 16)
            with zipfile.ZipFile(archive, "w") as bundle:
                bundle.writestr(link, "../outside")
            with self.assertRaisesRegex(RuntimeError, "Symbolic links"):
                updates.safe_extract_zip(archive, root / "out")

    def test_safe_extract_zip_extracts_regular_files(self):
        with tempfile.TemporaryDirectory() as temp_name:
            root = Path(temp_name)
            archive = root / "good.zip"
            with zipfile.ZipFile(archive, "w") as bundle:
                bundle.writestr("SSHMountMate/SSHMountMate.exe", "binary")
            updates.safe_extract_zip(archive, root / "out")
            self.assertEqual((root / "out" / "SSHMountMate" / "SSHMountMate.exe").read_text(), "binary")

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
