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

    def stopped(self):
        return {"LoadState": "loaded", "ActiveState": "inactive", "SubState": "dead",
                "MainPID": "0"}

    def test_authoritatively_inactive_is_absent(self):
        with mock.patch.object(rr, "_service_state", return_value=(self.stopped(), "")), \
             mock.patch.object(rr, "_bridge_pid", return_value=(None, "no store")):
            self.assertEqual(rr.evaluate("unused", "1", "unused")["state"], rr.ABSENT)

    RECORDED = "2026-08-27T12:00:00+00:00"

    def _recorded(self, started=RECORDED, pid=4242):
        return ((pid, started), "")

    def _stopped_with(self, record, kill, started_at=None):
        """systemd says stopped; vary what the store recorded and what the process looks like."""
        patches = [mock.patch.object(rr, "_service_state", return_value=(self.stopped(), "")),
                   mock.patch.object(rr, "_bridge_record", return_value=record),
                   mock.patch.object(rr, "_kill_probe", return_value=kill)]
        if started_at is not None:
            patches.append(mock.patch.object(rr, "_process_started_at", return_value=started_at))
        with patches[0], patches[1], patches[2]:
            if started_at is None:
                return rr.evaluate("unused", "1", "unused")
            with patches[3]:
                return rr.evaluate("unused", "1", "unused")

    def _epoch(self, offset=0):
        import datetime as dt
        return dt.datetime.fromisoformat(self.RECORDED).timestamp() + offset

    def test_a_bridge_running_OUTSIDE_the_unit_defeats_systemds_stopped(self):
        """The only branch that grants success may not rest on one witness. A bridge started by
        hand, or under another unit name, is invisible to systemd while holding every route in
        memory — and the store's recorded PID is the one piece of evidence independent of it."""
        st = self._stopped_with(self._recorded(), (True, ""), (self._epoch(-10), ""))
        self.assertEqual(st["state"], rr.UNKNOWN)
        self.assertIn("4242", st["reason"])

    def test_a_REUSED_pid_does_not_block_absent(self):
        """The other half, and the one that decides whether this converges. A lingering PID means
        an unclean exit, and that number is eventually handed to something unrelated. Treating
        the stranger as a bridge states something false and blocks a deprovision that had in fact
        finished — so the process is identified against the start time the bridge recorded."""
        st = self._stopped_with(self._recorded(), (True, ""), (self._epoch(3600), ""))
        self.assertEqual(st["state"], rr.ABSENT)

    def test_a_live_pid_that_cannot_be_identified_is_unknown(self):
        """No recorded start time, so the process cannot be tied to the record either way."""
        st = self._stopped_with(self._recorded(started=None), (True, ""))
        self.assertEqual(st["state"], rr.UNKNOWN)
        self.assertIn("could not be identified", st["reason"])

    def test_an_unprobeable_recorded_pid_also_defeats_stopped(self):
        """Unmeasured must not read as stopped."""
        st = self._stopped_with(self._recorded(), (None, "EPERM"))
        self.assertEqual(st["state"], rr.UNKNOWN)

    def test_a_dead_recorded_pid_does_not_block_absent(self):
        """POSITIVE CONTROL: the ordinary converged case must still reach ABSENT, or the tests
        above would pass against an authority that can never succeed."""
        st = self._stopped_with(self._recorded(), (False, "gone"))
        self.assertEqual(st["state"], rr.ABSENT)

    def test_a_cleanly_stopped_bridge_leaves_no_record_to_contradict(self):
        """The common path: the bridge compare-and-clears its PID on a clean stop, so there is
        nothing to probe and systemd's word stands."""
        st = self._stopped_with((None, "bridge PID metadata is missing"), (False, "unused"))
        self.assertEqual(st["state"], rr.ABSENT)


class ShippedUnit(unittest.TestCase):
    def test_the_default_unit_is_the_one_this_repository_ships(self):
        """THE DEFECT NO OTHER TEST COULD SEE. Every test above mocks `_service_state`, so the
        unit NAME is never exercised — and it was `multi-bridge.service` while the shipped unit
        is `bridge.service`. `systemctl show` exits 0 for an unknown unit with
        `LoadState=not-found`, so this surfaced as UNKNOWN on every run: deprovision could never
        converge on the serve host, and the message blamed a systemd that was fine."""
        root = pathlib.Path(__file__).resolve().parents[2]
        shipped = root / "multi" / "serve" / "bridge.service"
        self.assertTrue(shipped.is_file(), f"the shipped unit moved: {shipped}")
        self.assertEqual(rr.DEFAULT_UNIT, shipped.name)

    def test_nothing_still_names_the_unit_that_never_existed(self):
        root = pathlib.Path(__file__).resolve().parents[2]
        for rel in ("deploy/lib/route_revocation.py", "multi/provision/deprovision.sh"):
            body = (root / rel).read_text()
            code = "\n".join(ln for ln in body.splitlines() if not ln.lstrip().startswith("#"))
            self.assertNotIn("multi-bridge", code,
                             f"{rel} still refers to a unit this repository does not ship")


class Locale(unittest.TestCase):
    def test_subprocesses_run_under_a_pinned_locale(self):
        """`ps -o lstart=` renders month names in the caller's locale and
        `_process_started_at` parses them with English codes, so an operator under LC_TIME=de_DE
        got UNKNOWN forever — fail-safe, and unable to converge."""
        seen = {}
        real = rr.subprocess.run

        def spy(args, **kw):
            seen.update(kw.get("env") or {})
            return real([sys.executable, "-c", "print('x')"], **{**kw, "env": None})

        with mock.patch.object(rr.subprocess, "run", spy):
            rr._command(["ps", "-o", "lstart=", "-p", "1"])
        self.assertEqual(seen.get("LC_ALL"), "C")


if __name__ == "__main__":
    unittest.main()
