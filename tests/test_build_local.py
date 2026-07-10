from __future__ import annotations

import unittest
from pathlib import Path
from unittest import mock

from build import build_local


class BuildLocalTests(unittest.TestCase):
    def test_build_variant_selects_requested_pyinstaller_mode(self):
        with mock.patch.object(build_local.subprocess, "call", return_value=0) as call:
            result = build_local.build_variant(Path("/repo"), Path("/assets"), Path("/rclone"), "onedir")

        self.assertEqual(result, 0)
        command = call.call_args.args[0]
        self.assertIn("--onedir", command)
        self.assertNotIn("--onefile", command)
        dist_path = Path(command[command.index("--distpath") + 1])
        self.assertEqual(dist_path.parts[-2:], ("dist", "onedir"))


if __name__ == "__main__":
    unittest.main()
