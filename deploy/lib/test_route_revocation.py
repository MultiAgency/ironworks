#!/usr/bin/env python3
import contextlib
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
        return (rr.RECORDED, (pid, started), "")

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
        st = self._stopped_with(
            (rr.NO_RECORD, None, "the bridge store records no PID"), (False, "unused"))
        self.assertEqual(st["state"], rr.ABSENT)


class WithoutServiceManager(unittest.TestCase):
    """A host with no systemd still has to be able to converge.

    The bridge is an ordinary `python3 -u telegram_bridge.py`; a unit exists where systemd does,
    and on a developer machine it is started by hand. Returning UNKNOWN there made deprovision
    permanently DEGRADED with no operator action that could clear it — a gate people learn to
    ignore. This module's own host is such a machine, so the path is not hypothetical.
    """

    RECORDED = "2026-08-27T12:00:00+00:00"

    def _epoch(self, offset=0):
        import datetime as dt
        return dt.datetime.fromisoformat(self.RECORDED).timestamp() + offset

    def _no_systemd(self, record, kill=None, started_at=None, removed="1"):
        stack = [mock.patch.object(rr, "_bridge_record", return_value=record)]
        if kill is not None:
            stack.append(mock.patch.object(rr, "_kill_probe", return_value=kill))
        if started_at is not None:
            stack.append(mock.patch.object(rr, "_process_started_at",
                                           return_value=(started_at, "")))
        with contextlib.ExitStack() as es:
            for p in stack:
                es.enter_context(p)
            return rr.evaluate("unused", removed, "unused", rr.NO_SERVICE_MANAGER)

    def test_a_cleanly_stopped_bridge_is_ABSENT(self):
        """The pid is cleared on a clean stop, so its absence is positive evidence — which is
        exactly why "no record" and "cannot read the record" had to stop being the same value."""
        st = self._no_systemd((rr.NO_RECORD, None, "the bridge store records no PID"))
        self.assertEqual(st["state"], rr.ABSENT)
        self.assertEqual(st["authority"], "bridge-state")

    def test_an_unreadable_store_is_UNKNOWN_not_absent(self):
        st = self._no_systemd((rr.UNREADABLE, None, "bridge PID metadata is unreadable"))
        self.assertEqual(st["state"], rr.UNKNOWN)

    def test_a_live_bridge_older_than_the_removal_is_PRESENT(self):
        st = self._no_systemd((rr.RECORDED, (4242, self.RECORDED), ""), kill=(True, ""),
                              started_at=self._epoch(-10), removed=str(self._epoch(60)))
        self.assertEqual(st["state"], rr.PRESENT)

    def test_a_live_bridge_started_after_the_removal_is_ABSENT(self):
        st = self._no_systemd((rr.RECORDED, (4242, self.RECORDED), ""), kill=(True, ""),
                              started_at=self._epoch(-10), removed=str(self._epoch(-600)))
        self.assertEqual(st["state"], rr.ABSENT)

    def test_a_reused_pid_does_not_make_it_PRESENT(self):
        """The identity check is what survives losing systemd's MainPID corroboration."""
        st = self._no_systemd((rr.RECORDED, (4242, self.RECORDED), ""), kill=(True, ""),
                              started_at=self._epoch(3600), removed=str(self._epoch(60)))
        self.assertEqual(st["state"], rr.ABSENT)

    def test_a_dead_recorded_pid_is_ABSENT(self):
        st = self._no_systemd((rr.RECORDED, (4242, self.RECORDED), ""), kill=(False, "gone"))
        self.assertEqual(st["state"], rr.ABSENT)

    def test_every_verdict_names_which_authority_answered(self):
        """"systemd says stopped" and "nothing claims to be running" are different strengths of
        the same word, and an operator reading an audit line deserves to know which one it was."""
        st = self._no_systemd((rr.NO_RECORD, None, "no PID"))
        self.assertIn("BRIDGE_SERVICE_UNIT=none", st["reason"])

    def test_an_UNAVAILABLE_service_manager_is_still_UNKNOWN(self):
        """The declaration is what selects the weaker authority — never systemd's silence.
        Otherwise anything that breaks `systemctl` relaxes the check instead of failing it."""
        with mock.patch.object(rr, "_service_state", return_value=(None, "systemctl failed")):
            st = rr.evaluate("unused", "1", "unused")
        self.assertEqual(st["state"], rr.UNKNOWN)
        self.assertNotIn("authority", st)


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
