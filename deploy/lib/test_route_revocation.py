#!/usr/bin/env python3
import pathlib
import sys
import unittest
from unittest import mock

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import route_revocation as rr


class RouteAuthority(unittest.TestCase):
    def service(self, pid=41):
        return {"LoadState": "loaded", "ActiveState": "active", "SubState": "running",
                "MainPID": str(pid)}

    def test_eperm_is_unknown(self):
        with mock.patch.object(rr, "_service_state", return_value=(self.service(), "")), \
             mock.patch.object(rr, "_bridge_pid", return_value=(41, "")), \
             mock.patch.object(rr, "_kill_probe", return_value=(None, "permission denied (EPERM)")):
            st = rr.evaluate("unused", "1", "unused")
        self.assertEqual(st["state"], rr.UNKNOWN)
        self.assertIn("EPERM", st["reason"])

    def test_pid_reuse_or_service_metadata_contradiction_is_unknown(self):
        with mock.patch.object(rr, "_service_state", return_value=(self.service(42), "")), \
             mock.patch.object(rr, "_bridge_pid", return_value=(41, "")):
            st = rr.evaluate("unused", "1", "unused")
        self.assertEqual(st["state"], rr.UNKNOWN)
        self.assertIn("contradicts", st["reason"])

    def test_authoritatively_inactive_is_absent(self):
        service = {"LoadState": "loaded", "ActiveState": "inactive", "SubState": "dead",
                   "MainPID": "0"}
        with mock.patch.object(rr, "_service_state", return_value=(service, "")):
            self.assertEqual(rr.evaluate("unused", "1", "unused")["state"], rr.ABSENT)


if __name__ == "__main__":
    unittest.main()
