from __future__ import annotations

import unittest

import ssh_mountmate.__main__ as package_main
from ssh_mountmate import core, gui


class EntrypointTests(unittest.TestCase):
    def test_package_main_uses_gui_main(self):
        self.assertIs(package_main.main, gui.main)

    def test_legacy_core_cli_is_not_exposed(self):
        self.assertFalse(hasattr(core, "build_parser"))
        self.assertFalse(hasattr(core, "cmd_mount"))
        self.assertFalse(hasattr(core, "cmd_umount"))
        self.assertFalse(hasattr(core, "main"))


if __name__ == "__main__":
    unittest.main()
