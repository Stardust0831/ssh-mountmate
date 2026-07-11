from __future__ import annotations

import unittest
from unittest import mock

from ssh_mountmate import rclone_rc


class RcloneRcTests(unittest.TestCase):
    def test_rc_url_rejects_non_loopback_address(self):
        with self.assertRaisesRegex(RuntimeError, "non-loopback"):
            rclone_rc.rc_url("192.0.2.10:5572", "core/stats")

    def test_build_transfer_snapshot_combines_queue_and_live_bytes(self):
        snapshot = rclone_rc.build_transfer_snapshot(
            {"queue": [{"id": 7, "name": "data/big.bin", "size": 1000, "uploading": True, "tries": 1}]},
            {"diskCache": {"uploadsQueued": 1, "uploadsInProgress": 1, "erroredFiles": 0}},
            {"transferring": [{"name": "big.bin", "bytes": 400, "percentage": 40, "speedAvg": 25, "eta": 24}]},
        )

        self.assertEqual(snapshot["queued"], 1)
        self.assertEqual(snapshot["transferred_bytes"], 400)
        self.assertEqual(snapshot["percentage"], 40)
        self.assertFalse(snapshot["synced"])
        self.assertEqual(snapshot["files"][0]["speed"], 25)

    def test_build_transfer_snapshot_requires_empty_queue_for_synced(self):
        snapshot = rclone_rc.build_transfer_snapshot(
            {"queue": []},
            {"diskCache": {"uploadsQueued": 0, "uploadsInProgress": 0, "erroredFiles": 0}},
            {},
        )

        self.assertTrue(snapshot["synced"])
        self.assertEqual(snapshot["percentage"], 100)

    def test_refresh_remote_snapshot_forgets_refreshes_and_verifies(self):
        responses = [
            {"queue": [{"name": "pending"}]},
            {},
            {},
            {"list": [{"Path": "new.txt", "Size": 12}]},
        ]

        with mock.patch.object(rclone_rc, "rc_call", side_effect=responses) as call:
            result = rclone_rc.refresh_remote_snapshot("127.0.0.1:1234", "server:path", "subdir")

        self.assertEqual(result["pending_uploads"], 1)
        self.assertEqual(result["entries"][0]["Path"], "new.txt")
        self.assertEqual(call.call_args_list[1], mock.call("127.0.0.1:1234", "vfs/forget", {"dir": "subdir"}))
        self.assertEqual(call.call_args_list[2], mock.call("127.0.0.1:1234", "vfs/refresh", {"dir": "subdir"}))

    def test_transfer_name_matches_relative_and_full_paths(self):
        self.assertTrue(rclone_rc.transfer_matches("project/data.bin", "/remote/project/data.bin"))
        self.assertFalse(rclone_rc.transfer_matches("project/a.bin", "project/b.bin"))

    def test_process_id_uses_short_rc_timeout(self):
        with mock.patch.object(rclone_rc, "rc_call", return_value={"pid": 42}) as call:
            self.assertEqual(rclone_rc.process_id("127.0.0.1:1234"), 42)

        call.assert_called_once_with("127.0.0.1:1234", "core/pid", timeout=0.75)


if __name__ == "__main__":
    unittest.main()
