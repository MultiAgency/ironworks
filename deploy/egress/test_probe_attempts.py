#!/usr/bin/env python3
"""The egress probe's positive control: does a forbidden leg measure anything?

Run: python3 deploy/egress/test_probe_attempts.py

Offline and stdlib-only, because the thing under test decides whether the live probe is allowed
to write a VERIFIED stamp, and the live probe needs docker and a provider key. A check that
CANNOT pass and a check that is merely failing look identical from the probe's output, so the
discrimination is tested where it can be driven directly.

Three entries in `forbidden-destinations.json` asserted a negative against an address that could
never have been reached from anywhere — two `127.0.0.1` legs that named the runtime's own
loopback instead of the docker host, and a `host.docker.internal` leg that failed at DNS. Each
returned "blocked" against a container with completely unrestricted egress and was counted into
the assertion total written to the stamp. `measurable()` is the line those fell on the wrong
side of.
"""
import errno
import json
import pathlib
import socket
import sys
import unittest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

import probe_attempts as pa  # noqa: E402

HERE = pathlib.Path(__file__).resolve().parent


class _Sock:
    def close(self):
        pass


def raising(exc):
    def _c(*_args, **_kwargs):
        raise exc
    return _c


class Classify(unittest.TestCase):
    def setUp(self):
        self._real = socket.create_connection
        self.addCleanup(lambda: setattr(socket, "create_connection", self._real))

    def outcome(self, exc_or_none):
        socket.create_connection = (raising(exc_or_none) if exc_or_none
                                    else (lambda *a, **k: _Sock()))
        return pa.classify("h.example", 443)

    def test_a_completed_handshake_is_DISCRIMINATING(self):
        """The only outcome that is evidence: it connected with full egress, so the same attempt
        failing inside the boundary is the boundary doing it."""
        verdict, why = self.outcome(None)
        self.assertEqual(verdict, pa.DISCRIMINATING)
        self.assertIn("connected", why)

    def test_a_timeout_is_CORROBORATING_not_evidence(self):
        """THE REGRESSION THIS CLASS EXISTS FOR. An earlier version called this "measurable" on
        the reasoning that the packet had somewhere to go, so a containment failure "WOULD have
        shown up as a connection". It would not: the attempt times out with the boundary present
        and absent alike. Run the contained probe body against a container on the default bridge
        with no gateway and the timeout legs — cloud metadata, link-local, 10/8, 172.16/12 — all
        score PASS with no boundary whatsoever, and were counted into the number the verification
        stamp reports as "assertions proved"."""
        verdict, why = self.outcome(socket.timeout("timed out"))
        self.assertEqual(verdict, pa.CORROBORATING)
        self.assertIn("same way either way", why)

    def test_an_active_refusal_is_CORROBORATING_not_evidence(self):
        """Something answered the SYN with a RST — but a contained container gets a failure too,
        so this leg cannot tell the two apart either. `host.docker.internal:5432` is the live
        example: the database refuses from the bridge and is unreachable from inside."""
        verdict, why = self.outcome(OSError(errno.ECONNREFUSED, "refused"))
        self.assertEqual(verdict, pa.CORROBORATING)
        self.assertIn("same way either way", why)

    def test_corroborating_is_still_ASSERTED_though_never_counted(self):
        """The reason the middle state exists rather than being folded into UNMEASURABLE:
        requiring a handshake to assert at all would drop `10.0.0.1:8443` and `172.16.0.1:80` —
        which name ranges, not services — from the probe entirely on every ordinary host. They
        stay asserted, because a REACHED there is a genuine failure; they are not evidence."""
        shell = (HERE / "probe-egress.sh").read_text()
        self.assertIn("proves=pair in DISCRIMINATING", shell,
                      "the contained probe no longer distinguishes evidence from corroboration")
        self.assertIn("corroborating.append", shell,
                      "corroborating legs are not being retained as assertions")

    def test_a_name_that_does_not_resolve_is_UNMEASURABLE(self):
        """`host.docker.internal` with nothing mapping it. The attempt died at DNS, before the
        boundary could have had any say."""
        verdict, why = self.outcome(socket.gaierror(-2, "Name or service not known"))
        self.assertEqual(verdict, pa.UNMEASURABLE)
        self.assertIn("does not resolve", why)

    def test_no_route_is_UNMEASURABLE(self):
        for code in (errno.ENETUNREACH, errno.EHOSTUNREACH):
            verdict, why = self.outcome(OSError(code, "unreachable"))
            self.assertEqual(verdict, pa.UNMEASURABLE, errno.errorcode[code])
            self.assertIn("no route", why)

    def test_the_run_refuses_to_stamp_when_nothing_discriminates(self):
        """A host whose own outbound is firewalled times out everywhere, so every forbidden leg
        passes for the wrong reason. That run has measured the gateway and nothing else."""
        shell = (HERE / "probe-egress.sh").read_text()
        self.assertIn("if not DISCRIMINATING:", shell,
                      "a run in which no forbidden leg can discriminate would still stamp")


class Manifest(unittest.TestCase):
    def test_no_forbidden_destination_names_loopback(self):
        """THE REGRESSION. Both probes attempt this list from inside `--network container:`,
        where `127.0.0.1` is the TARGET CONTAINER's loopback and never the docker host. Two
        entries named it and were labelled as the Account Service and its database, so what they
        actually asserted was that the runtime does not serve Postgres to itself — true on every
        host, contained or not."""
        doc = json.loads((HERE / "forbidden-destinations.json").read_text())
        loopback = [d for d in doc["destinations"]
                    if d["host"] in ("127.0.0.1", "::1", "localhost")]
        self.assertEqual(
            loopback, [],
            "a forbidden destination names loopback, which inside `--network container:` is the "
            "runtime's own — it can never reach the docker host and the leg proves nothing")

    def test_every_destination_carries_a_label_and_a_port(self):
        doc = json.loads((HERE / "forbidden-destinations.json").read_text())
        self.assertTrue(doc["destinations"], "an empty forbidden list asserts nothing")
        for d in doc["destinations"]:
            self.assertTrue(d.get("label"), d)
            self.assertIsInstance(d.get("port"), int, d)
            self.assertTrue(d.get("host"), d)


if __name__ == "__main__":
    unittest.main(verbosity=2)
