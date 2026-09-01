#!/usr/bin/env python3
"""Both egress probes assert the same forbidden destinations. Offline, stdlib only.

WHAT THIS GUARDS. There are two probes of the same boundary:

  deploy/egress/probe-egress.sh          runs against a LIVE container, and a PASS writes the
                                         verification stamp `ironworks doctor` reports.
  deploy/egress/proof/proof_checks.py    runs against a disposable stack under run-proof.sh.

They carried separate destination lists, and the certifying one was the weaker: four
destinations against ten. The six it did not assert were the ones that matter on a real host —
plain HTTP, the 172.16/12 range, link-local, and the private Account Service by two names plus
its database. A runtime that could reach the Account Service would have been stamped VERIFIED.

The list is now one file. These tests fail if a probe stops reading it, which is the only way
the two can drift apart again.

AND SHARING A LIST IS NOT THE SAME AS ASSERTING IT. Three of the ten named addresses no probe
could reach from where it ran — two `127.0.0.1` legs that meant the docker host but named the
runtime's own loopback, and a `host.docker.internal` leg with nothing mapping that name — so
they returned "blocked" against a container with completely unrestricted egress and were counted
into the assertion total written to the stamp. Coverage was measured by presence in this file,
which is exactly the check that could not see it. The last two tests below are the ones that can.
"""
import json
import os
import pathlib
import re
import subprocess
import sys
import tempfile
import unittest

from egress_destinations import load_forbidden_destinations

ROOT = pathlib.Path(__file__).resolve().parents[2]
LIST = ROOT / "deploy" / "egress" / "forbidden-destinations.json"
SHELL_PROBE = ROOT / "deploy" / "egress" / "probe-egress.sh"
STACK_PROBE = ROOT / "deploy" / "egress" / "proof" / "proof_checks.py"
# The CONTAINED leg of the shell probe. It was 157 lines inlined in probe-egress.sh, so the
# assertions below read it out of the shell text; it is a file now and they read the file.
CONTAINED_PROBE = ROOT / "deploy" / "egress" / "probe_contained.py"
OVERLAY = ROOT / "deploy" / "egress" / "docker-compose.egress.yml"
PROOF_COMPOSE = ROOT / "deploy" / "egress" / "proof" / "docker-compose.proof.yml"


class ForbiddenDestinations(unittest.TestCase):
    def setUp(self):
        self.doc = json.loads(LIST.read_text())
        self.dests = self.doc["destinations"]

    def test_the_list_is_well_formed_and_not_empty(self):
        """An empty or malformed list would make both probes assert nothing and both pass.

        ASSERTED THROUGH THE PRODUCTION READER, not beside it. This test used to restate the
        schema — `set(d) == {label, host, port}`, port is an int — which meant it could agree with
        the file while disagreeing with the loader the probes actually run. Calling
        `load_forbidden_destinations` makes the two impossible to separate."""
        self.assertEqual(load_forbidden_destinations(ROOT), self.dests)
        self.assertGreater(len(self.dests), 0)

    def test_the_reader_refuses_every_shape_that_would_make_a_probe_vacuous(self):
        """The negative half, which neither restatement had: a schema check that only ever sees
        the valid committed file cannot show that it would REJECT anything."""
        for bad in ({}, {"destinations": []}, {"destinations": [{"label": "x", "host": "h"}]},
                    {"destinations": [{"label": "x", "host": "h", "port": "443"}]},
                    {"destinations": [{"label": "", "host": "h", "port": 443}]},
                    {"destinations": [{"label": "x", "host": "", "port": 443}]},
                    {"destinations": [{"label": "x", "host": "h", "port": 443, "extra": 1}]}):
            with tempfile.TemporaryDirectory() as d:
                root = pathlib.Path(d)
                (root / "deploy" / "egress").mkdir(parents=True)
                (root / "deploy" / "egress" / "forbidden-destinations.json").write_text(
                    json.dumps(bad))
                with self.assertRaises(ValueError, msg=bad):
                    load_forbidden_destinations(root)

    def test_it_still_covers_the_destinations_that_hold_client_data(self):
        """The six the stamping probe used to miss. Named explicitly, because losing one again
        would leave both probes green and the boundary unproven where it matters most.

        THE ACCOUNT SERVICE AND ITS DATABASE MOVED ADDRESS, and this list moved with them. They
        were `127.0.0.1:8443` and `127.0.0.1:5432`, attempted from inside `--network
        container:<runtime>` — where loopback is the RUNTIME's own and never the docker host. So
        the coverage this test protects was nominal: both legs returned "blocked" against a
        container with completely unrestricted egress. `host.docker.internal`, made resolvable by
        `--add-host=host.docker.internal:host-gateway` in both probes, is the address a container
        can actually use for the docker host. The service and the database are still each
        asserted; only the way of naming them is now one that can fail."""
        pairs = {(d["host"], d["port"]) for d in self.dests}
        for host, port, why in (
            ("host.docker.internal", 8443, "the private Account Service, by docker host name"),
            ("host.docker.internal", 5432, "the account database, by docker host name"),
            ("169.254.169.254", 80, "cloud metadata"),
            ("169.254.1.1", 80, "link-local"),
            ("172.16.0.1", 80, "the 172.16/12 private range"),
            ("10.0.0.1", 8443, "the 10/8 private range"),
            ("example.com", 80, "plain HTTP, not only HTTPS"),
        ):
            self.assertIn((host, port), pairs, f"no longer asserted: {why}")

    def test_the_RUNTIME_makes_the_docker_host_addressable(self):
        """The mapping belongs to the container under test, not to the probe.

        `host.docker.internal` is a docker convenience name, not DNS; on Linux — what the serve
        host runs — it does not resolve inside a container unless something maps it. The obvious
        fix, an `--add-host` flag on the probe, is IMPOSSIBLE: the probe joins the runtime's
        network namespace with `--network container:<target>`, and docker refuses the two
        together — "conflicting options: custom host-to-IP mapping and the network mode". The
        joining container shares the target's networking, hosts file included, so the target is
        the only place the name can be mapped.
        """
        for compose, service in ((OVERLAY, "ironclaw"), (PROOF_COMPOSE, "ic")):
            self.assertIn("host.docker.internal:host-gateway", compose.read_text(),
                          f"{compose.name} does not map the docker host for {service}, so the "
                          "Account Service and database legs cannot resolve and would pass "
                          "without measuring anything")

    def test_the_probe_does_not_pass_add_host_alongside_container_networking(self):
        """The combination docker rejects. It made the CONTAINED probe — the only mode that
        stamps — exit 125 on every run, so `egress-control.sh verify` could never succeed and
        nothing could reach VERIFIED again."""
        shell = SHELL_PROBE.read_text()
        contained = shell.split("docker run --rm \"${NETNS[@]}\"", 1)
        self.assertEqual(len(contained), 2, "the contained probe invocation moved or was renamed")
        invocation = contained[1].split("\n\n", 1)[0]
        self.assertNotIn("HOSTMAP", invocation,
                         "the contained probe passes --add-host alongside --network container:, "
                         "which docker refuses outright (exit 125)")

    def test_resolution_failure_inside_the_boundary_is_still_containment(self):
        """A leg is meaningless when it fails the SAME WAY from outside the boundary, which is
        what the control run decides — not when it happens to fail at DNS.

        Having no resolver path out is part of the boundary; the stack proof asserts it in its
        own right ("DNS cannot be abused to bypass the destination policy"). Measured against the
        live contained runtime, even `example.com` does not resolve inside `multi_inner`, so
        special-casing resolution failure marked five correct assertions BLOCKED and refused to
        stamp a boundary that was demonstrably working."""
        source = CONTAINED_PROBE.read_text()
        direct = source.split("def direct(", 1)[1].split("\ndef ", 1)[0]
        self.assertNotIn("gaierror", direct,
                         "direct() special-cases DNS failure again — that marks the boundary's "
                         "own lack of a resolver path as an unmeasured leg")
        self.assertIn("MEASURABLE", source,
                      "the control run is what decides whether a leg means anything")

    def test_a_forbidden_leg_is_only_counted_if_it_could_be_measured(self):
        """The positive control. A negative check against an address nothing can reach is
        satisfied by an empty network as readily as by a working boundary."""
        shell = SHELL_PROBE.read_text()
        self.assertIn("REACHABLE_FROM_UNCONTAINED", shell,
                      "probe-egress.sh no longer runs the uncontained control")
        self.assertIn("probe_attempts.py", shell)
        self.assertIn("unmeasured", CONTAINED_PROBE.read_text(),
                      "the contained probe does not separate UNMEASURED from passed")

    def test_both_probes_read_the_shared_list(self):
        """Neither may go back to its own literals. Checked by source inspection because one
        probe is a shell script embedding Python and the other runs inside a disposable stack —
        there is no process here that can import them both."""
        shell = SHELL_PROBE.read_text()
        self.assertIn("forbidden-destinations.json", shell,
                      "probe-egress.sh no longer reads the shared list")
        self.assertIn('json.loads(os.environ["FORBIDDEN_JSON"])', CONTAINED_PROBE.read_text(),
                      "the contained probe does not iterate the shared list")

        stack = STACK_PROBE.read_text()
        self.assertIn("forbidden-destinations.json", stack,
                      "proof_checks.py no longer reads the shared list")

    def test_neither_probe_hardcodes_a_destination_beside_the_list(self):
        """A literal `direct(...)` with an address in it is how the two lists grew apart. The
        provider host is the exception: it is the one destination that must be REACHABLE, and it
        is configuration (PROVIDER_HOST), not part of the forbidden set."""
        addr = re.compile(r'direct\([^)]*["\'](\d{1,3}(?:\.\d{1,3}){3}|[a-z0-9.-]+\.(?:com|example|internal))["\']')
        # CONTAINED_PROBE is in this tuple because every `direct(` call moved into it. Scanning
        # only the shell would still pass — over a file that no longer contains a single call
        # this pattern could match. A scan whose subject has moved is a scan of nothing.
        probes = (SHELL_PROBE, STACK_PROBE, CONTAINED_PROBE)
        calls = sum(p.read_text().count("direct(") for p in probes)
        self.assertGreater(calls, 3,
                           f"only {calls} `direct(` call(s) across {[p.name for p in probes]} — "
                           "this scan has lost its subject and would pass over anything")
        for probe in probes:
            for m in addr.finditer(probe.read_text()):
                self.fail(f"{probe.name} hardcodes destination {m.group(1)!r} outside the "
                          "shared list — add it to forbidden-destinations.json instead")

    def test_disposable_proof_configuration_executes_offline(self):
        """The staged export can load the proof contract without Docker, credentials, or live
        runtime state. Missing policy/support artifacts therefore fail in CI, not on a host."""
        env = {"PATH": os.environ.get("PATH", ""), "PYTHONDONTWRITEBYTECODE": "1"}
        run = subprocess.run([sys.executable, str(STACK_PROBE), "--offline-config"],
                             cwd=ROOT, env=env, capture_output=True, text=True)
        self.assertEqual(0, run.returncode, run.stdout + run.stderr)
        self.assertIn("PASS egress proof configuration loads", run.stdout)


if __name__ == "__main__":
    unittest.main(verbosity=2)
