#!/usr/bin/env python3
"""The egress boundary's state machine. Offline, stdlib only, no docker.

Run: python3 deploy/lib/test_egress_status.py

The live proof (deploy/egress/proof/) answers "does the boundary work?". This answers the
question an operator asks every day afterwards: "is it on, and does anything notice when it
isn't?" Those fail differently. A boundary that works but is reported as present when it is
absent is worse than no boundary, because it converts a known gap into an assumed guarantee.

Docker is faked at the `_docker` seam, so every state is reachable deterministically — including
the ones that would otherwise need someone to break production to observe.
"""
import json
import os
import pathlib
import subprocess
import sys
import tempfile
import time
import unittest

import egress_status as es

IMAGE = "sha256:abc123"
_CURRENT = object()   # sentinel: "the fingerprint this tree actually produces"
# What both the gateway enforces and the stamp records, in the baseline. They must match by
# default or every VERIFIED test would be asserting the allowlist-drift path by accident.
ALLOW = "cloud-api.near.ai:443"
STARTED = "2026-08-27T12:00:00.000000000Z"
LOGGED = "2026-08-27T12:00:01.000000000Z"


def contained_docker(gateway_present=True, image=IMAGE, running=True, gateway_running=True,
                     gateway_allow=ALLOW, gateway_impl=_CURRENT):
    """The CONTAINED baseline: the runtime on one internal network, gateway up — the shape that
    evaluates to RUNNING. Seven tests below set exactly this and then varied one other thing, so
    the configuration was retyped seven times and none of them said what it meant."""
    return fake_docker({"ic": ["inner"]}, {"inner": True}, gateway_present, image=image,
                       running=running, gateway_running=gateway_running,
                       gateway_allow=gateway_allow, gateway_impl=gateway_impl)


def fake_docker(networks, internal_map, gateway_present, image=IMAGE,
                running=True, gateway_running=True, gateway_allow=ALLOW,
                gateway_impl=_CURRENT, gateway_networks=None, network_ids=None):
    """A `_docker` stand-in: containers, their networks, and which networks are internal.

    `running` and `gateway_running` are separate from presence because `docker inspect` on an
    EXITED container exits 0 with its networks still populated — the state this fake could not
    express, which is why the suite passed while `evaluate` certified a stopped gateway.

    `gateway_allow` is what the GATEWAY enforces, which is a different fact from what the stamp
    records — keeping them separate is the whole point, since a boundary that widened after it
    was proved is exactly the case the stamp is supposed to catch. `None` means the environment
    could not be read at all, which must not be merged with "no allowlist"."""
    impl = (es._sha256_of(es._REPO / "deploy/egress/connect-proxy.py")
            if gateway_impl is _CURRENT else gateway_impl)

    def _d(*args):
        if args[0] == "version":
            return 0, "ok", ""
        # `docker logs <gateway>` — where the running proxy announces the bytes it loaded.
        # `None` models a container started before that banner existed.
        if args[0] == "logs":
            if args[-1] != es.os.environ.get("EGRESS_GATEWAY", "multi-egress-1"):
                return 1, "", "No such container"
            if not gateway_present or impl is None:
                return 0, f"{LOGGED} egress gateway on 0.0.0.0:3128", ""
            return 0, (f"{LOGGED} impl sha256={impl}\n"
                       f"{LOGGED} egress gateway on 0.0.0.0:3128"), ""
        # `docker inspect <name> -f <format>`: the gateway's own environment, which is where the
        # LIVE allowlist lives. Answered before the plain-inspect branch because it is the same
        # verb with more arguments.
        if args[0] == "inspect" and len(args) == 4 and args[2] == "-f":
            if args[1] != es.os.environ.get("EGRESS_GATEWAY", "multi-egress-1"):
                return 1, "", "No such object"
            if not gateway_present or gateway_allow is None:
                return 1, "", "No such object"
            return 0, f"PATH=/usr/bin\nEGRESS_ALLOW={gateway_allow}\nEGRESS_PORT=3128", ""
        if args[0] == "inspect" and len(args) == 2:
            name = args[1]
            if name in networks:
                return 0, json.dumps([{"NetworkSettings": {
                    "Networks": {n: {} for n in networks[name]}}, "Image": image,
                    "State": {"Running": running, "StartedAt": STARTED}}]), ""
            if name == es.os.environ.get("EGRESS_GATEWAY", "multi-egress-1"):
                gw_nets = gateway_networks if gateway_networks is not None else ["inner"]
                return (0, json.dumps([{"NetworkSettings": {
                                        "Networks": {n: {} for n in gw_nets}}, "Image": image,
                                        "State": {"Running": gateway_running,
                                                  "StartedAt": STARTED}}]), "") \
                    if gateway_present else (1, "", "no")
            return 1, "", "No such object"
        if args[0] == "network" and args[1] == "inspect":
            n = args[2]
            if n not in internal_map:
                return 1, "", "no such network"
            network_id = (network_ids or {}).get(n, "network-id-" + n)
            return 0, json.dumps([{"Id": network_id, "Internal": internal_map[n]}]), ""
        return 1, "", "?"
    return _d


class Base(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self._saved = {k: os.environ.get(k) for k in
                       ("EGRESS_STAMP", "EGRESS_DEGRADED_MARK", "EGRESS_GATEWAY")}
        os.environ["EGRESS_STAMP"] = str(pathlib.Path(self.tmp.name) / "stamp.json")
        os.environ["EGRESS_DEGRADED_MARK"] = str(pathlib.Path(self.tmp.name) / "degraded.json")
        os.environ["EGRESS_GATEWAY"] = "gw"
        self._real = es._docker

    def tearDown(self):
        es._docker = self._real
        for k, v in self._saved.items():
            if v is None:
                os.environ.pop(k, None)
            else:
                os.environ[k] = v
        self.tmp.cleanup()

    def stamp(self, container="ic", image=IMAGE, age=0, allow=ALLOW, fingerprint=_CURRENT):
        """A stamp as a PASSING probe would have written it.

        `fingerprint` defaults to the real one for the current tree and the fake gateway image,
        because a stamp that does not name the proof it earned is legacy by definition and must
        not be VERIFIED — which is what every test here would otherwise be asserting by accident.
        Pass `None` for a legacy stamp, or any other value for one from a different proof.
        """
        doc = {"container": container, "image_id": image, "allow": allow,
               "checks_passed": 42, "at": int(time.time()) - age, "at_iso": "then",
               "gateway_image": IMAGE, "proof_contract": es.PROOF_CONTRACT}
        topology, why = es.current_topology(container, os.environ["EGRESS_GATEWAY"])
        if topology is None and container != "ic":
            # This fixture intentionally names the wrong container; its topology is never
            # consulted because the container binding rejects it first.
            topology, why = es.current_topology("ic", os.environ["EGRESS_GATEWAY"])
        if topology is None:
            raise AssertionError(why)
        doc["topology"] = topology
        fp = es.proof_fingerprint(IMAGE) if fingerprint is _CURRENT else fingerprint
        if fp is not None:
            doc["proof_fingerprint"] = fp
        pathlib.Path(os.environ["EGRESS_STAMP"]).write_text(json.dumps(doc))


class DecisionLog(unittest.TestCase):
    """`decision_lines` — the parse the egress proof's "is this the right gateway's log?" check
    rests on. No docker, because a check that CANNOT PASS and a check that is merely failing are
    indistinguishable from the proof's output, and the proof needs a live key to run at all."""

    # Exactly what `docker compose logs gw` emits: service name, padding, pipe, space.
    COMPOSE = ("gw-1  | EGRESS_ALLOW=cloud-api.near.ai:443\n"
               "gw-1  | allow cloud-api.near.ai:443\n"
               "gw-1  | DENY  example.com:443 — not in the allowlist (1 entr(ies))\n"
               "gw-1  | fail  cloud-api.near.ai:443 — upstream unreachable (OSError)\n")

    def test_the_compose_prefix_is_stripped_before_the_verb_is_read(self):
        """THE REGRESSION. Splitting the raw line yields `gw-1`, so the decision list came out
        empty on every run and the check that asserts on it could never pass."""
        found = es.decision_lines(self.COMPOSE)
        self.assertEqual(len(found), 3, found)
        self.assertTrue(any("cloud-api.near.ai:443" in ln for ln in found))
        self.assertTrue(found[0].startswith("allow"), found[0])

    def test_the_startup_banner_is_not_a_decision(self):
        """The reason the check reads decisions rather than `ALLOW_HOST in logs`: the gateway
        names its allowlist at boot, so the weaker test is satisfied by a gateway that served
        nothing."""
        banner = "gw-1  | EGRESS_ALLOW=cloud-api.near.ai:443\n"
        self.assertEqual(es.decision_lines(banner), [])

    def test_unprefixed_output_still_parses(self):
        """`--no-log-prefix`, or a plain `docker logs`. Stripping must be tolerant, not required."""
        self.assertEqual(len(es.decision_lines("allow a.example:443\ndeny b.example:443\n")), 2)

    def test_empty_and_failed_log_reads_yield_nothing(self):
        for empty in ("", None, "\n\n"):
            self.assertEqual(es.decision_lines(empty), [], repr(empty))


class States(Base):
    def test_a_routed_network_is_FAILED_however_good_the_overlay_looks(self):
        """The defect this whole module exists to prevent: an overlay edited but never
        recreated. The file is perfect; the container still has a default route."""
        es._docker = fake_docker({"ic": ["multi_default"]}, {"multi_default": False}, True)
        st = es.evaluate("ic")
        self.assertEqual(st["state"], es.FAILED)
        self.assertIn("default route", " ".join(st["why"]))

    def test_internal_only_with_a_live_gateway_but_no_proof_is_RUNNING_not_VERIFIED(self):
        es._docker = contained_docker()
        st = es.evaluate("ic")
        self.assertEqual(st["state"], es.RUNNING)
        self.assertIn("no verification stamp", " ".join(st["why"]))

    def test_a_passing_probe_against_this_image_makes_it_VERIFIED(self):
        es._docker = contained_docker()
        self.stamp()
        self.assertEqual(es.evaluate("ic")["state"], es.VERIFIED)

    def test_a_stamp_for_a_DIFFERENT_image_does_not_count(self):
        """A pin bump changes the tool taxonomy and the HTTP client. An inherited VERIFIED is
        the most dangerous kind of stale, so the stamp is bound to the image id."""
        es._docker = contained_docker()
        self.stamp(image="sha256:something-else")
        st = es.evaluate("ic")
        self.assertEqual(st["state"], es.RUNNING)
        self.assertIn("DIFFERENT image", " ".join(st["why"]))

    def test_an_old_stamp_expires(self):
        es._docker = contained_docker()
        self.stamp(age=es.STAMP_MAX_AGE_SECONDS + 60)
        self.assertEqual(es.evaluate("ic")["state"], es.RUNNING)

    def test_a_stamp_for_another_container_does_not_count(self):
        es._docker = contained_docker()
        self.stamp(container="some-other-container")
        self.assertEqual(es.evaluate("ic")["state"], es.RUNNING)

    def test_a_WIDENED_gateway_allowlist_invalidates_the_stamp(self):
        """THE ADVERSARIAL CASE. Everything the stamp used to check is untouched: same runtime
        container, same image id, same age. Only the gateway's own `EGRESS_ALLOW` grew, and
        recreating the `egress` service alone does not change the runtime's image — so before
        this, `doctor` reported VERIFIED for a boundary that now permits an exfiltration host.
        `probe-egress.sh` records the list the gateway enforces precisely so this is checkable;
        nothing read it back."""
        es._docker = contained_docker(
            gateway_allow="cloud-api.near.ai:443,exfil.attacker.net:443")
        self.stamp()
        st = es.evaluate("ic")
        self.assertEqual(st["state"], es.RUNNING)
        why = " ".join(st["why"])
        self.assertIn("allowlist has CHANGED", why)
        self.assertIn("exfil.attacker.net:443", why, "the operator is not told WHAT was added")

    def test_a_NARROWED_gateway_allowlist_also_invalidates_the_stamp(self):
        """Not a security regression, but the stamp still describes a boundary that is not the
        one running, and a probe that passed against two destinations proves nothing about one.
        Reported as changed, with the direction named."""
        es._docker = contained_docker(gateway_allow=ALLOW)
        self.stamp(allow=f"{ALLOW},api.example:443")
        st = es.evaluate("ic")
        self.assertEqual(st["state"], es.RUNNING)
        self.assertIn("no longer permits: api.example:443", " ".join(st["why"]))

    def test_reordering_and_spacing_are_not_a_change(self):
        """The POSITIVE CONTROL for the two above: the comparison must be the set the proxy
        enforces, not the string. A false alarm here would train an operator to ignore the real
        one, and `connect-proxy.py` normalises exactly this way."""
        es._docker = contained_docker(gateway_allow=" B.example:443 , a.example:443")
        self.stamp(allow="a.example:443,b.example:443")
        self.assertEqual(es.evaluate("ic")["state"], es.VERIFIED)

    def test_an_unreadable_gateway_environment_is_not_VERIFIED(self):
        """"Could not check" must never read as "fine" — this module's founding rule. The
        gateway is up and the runtime is contained, so the state is RUNNING, but nothing can
        confirm the stamp describes the boundary now enforcing."""
        es._docker = contained_docker(gateway_allow=None)
        self.stamp()
        st = es.evaluate("ic")
        self.assertEqual(st["state"], es.RUNNING)
        self.assertIn("could not be read", " ".join(st["why"]))

    def test_gateway_allowlist_reads_the_live_environment(self):
        es._docker = contained_docker(gateway_allow="a.example:443")
        self.assertEqual(es.gateway_allowlist("gw"), "a.example:443")
        es._docker = contained_docker(gateway_allow=None)
        self.assertIsNone(es.gateway_allowlist("gw"),
                          "an unreadable environment must be None, never an empty allowlist")

    def test_a_stamp_with_NO_proof_fingerprint_is_not_VERIFIED(self):
        """THE LEGACY CASE, and it was live. The operator's own stamp recorded
        `checks_passed: 0` — written by a probe that had no uncontained control and against a
        destination manifest that has since gained and lost entries — and still read VERIFIED
        after the proof was corrected. A stamp is evidence about a PROCEDURE; one that does not
        name its procedure cannot certify this one."""
        es._docker = contained_docker()
        self.stamp(fingerprint=None)
        st = es.evaluate("ic")
        self.assertEqual(st["state"], es.RUNNING)
        self.assertIn("predates proof fingerprinting", " ".join(st["why"]))

    def test_a_changed_proof_definition_invalidates_the_stamp(self):
        """Edit what the boundary enforces, which destinations are asserted, or how a leg is
        counted, and the old result attests to a procedure that no longer exists."""
        es._docker = contained_docker()
        self.stamp(fingerprint="0" * 64)
        st = es.evaluate("ic")
        self.assertEqual(st["state"], es.RUNNING)
        self.assertIn("EGRESS PROOF ITSELF has changed", " ".join(st["why"]))

    def test_replacing_the_GATEWAY_image_invalidates_the_stamp(self):
        """The proxy is half of what was proved. The runtime image is untouched here, so every
        older check passes — and the container actually enforcing the boundary was swapped."""
        es._docker = contained_docker()
        self.stamp(fingerprint=es.proof_fingerprint("sha256:a-different-gateway"))
        st = es.evaluate("ic")
        self.assertEqual(st["state"], es.RUNNING)
        self.assertIn("EGRESS PROOF ITSELF has changed", " ".join(st["why"]))

    def test_an_unreadable_proof_definition_is_not_VERIFIED(self):
        """"Could not check" is never "fine" — this module's founding rule, applied to the proof
        itself rather than to the boundary."""
        es._docker = contained_docker()
        self.stamp()
        real = es.proof_fingerprint
        es.proof_fingerprint = lambda *a, **k: None
        try:
            st = es.evaluate("ic")
        finally:
            es.proof_fingerprint = real
        self.assertEqual(st["state"], es.RUNNING)
        self.assertIn("could not be read", " ".join(st["why"]))

    def test_the_fingerprint_moves_with_each_material_input(self):
        """Every input is load-bearing: if one of these can change without moving the hash, a
        stamp survives a change to the thing it certifies."""
        base = es.proof_fingerprint(IMAGE)
        self.assertEqual(base, es.proof_fingerprint(IMAGE), "the fingerprint is not stable")
        self.assertNotEqual(base, es.proof_fingerprint("sha256:other"), "gateway image")
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            for rel in es._PROOF_INPUTS:
                (root / rel).parent.mkdir(parents=True, exist_ok=True)
                (root / rel).write_bytes((es._REPO / rel).read_bytes())
            copied = es.proof_fingerprint(IMAGE, root=root)
            self.assertEqual(copied, base, "an identical copy produced a different fingerprint")
            for rel in es._PROOF_INPUTS:
                original = (root / rel).read_bytes()
                (root / rel).write_bytes(original + b"\n# changed\n")
                self.assertNotEqual(es.proof_fingerprint(IMAGE, root=root), base,
                                    f"editing {rel} did not move the fingerprint")
                (root / rel).write_bytes(original)
            (root / es._PROOF_INPUTS[0]).unlink()
            self.assertIsNone(es.proof_fingerprint(IMAGE, root=root),
                              "an unreadable input produced a hash of partial input")

    def test_a_gateway_running_DIFFERENT_bytes_is_not_VERIFIED(self):
        """THE BIND-MOUNT GAP. The gateway is a generic pinned base image with connect-proxy.py
        mounted in, so its image id never moves when the implementation does and the container
        keeps serving whatever it started with. Edit the file, skip the recreate, re-run the
        probe: every other check passes and the stamp records the NEW hash while the OLD proxy is
        what enforces the boundary. The stamp would certify a proxy that is not loaded."""
        es._docker = contained_docker(gateway_impl="b" * 64)
        self.stamp()
        st = es.evaluate("ic")
        self.assertEqual(st["state"], es.RUNNING)
        why = " ".join(st["why"])
        self.assertIn("RUNNING gateway is executing a different connect-proxy.py", why)
        self.assertIn("recreate the egress service", why)

    def test_a_gateway_that_reports_no_implementation_is_not_VERIFIED(self):
        """A container started before the gateway announced its own identity. Unmeasured is not
        fine — this module's founding rule, applied to the proxy instead of the boundary."""
        es._docker = contained_docker(gateway_impl=None)
        self.stamp()
        st = es.evaluate("ic")
        self.assertEqual(st["state"], es.RUNNING)
        self.assertIn("did not report the implementation it loaded", " ".join(st["why"]))

    def test_a_HARMLESS_RESTART_does_not_invalidate_the_proof(self):
        """THE OTHER HALF OF THE INVARIANT, and the reason no volatile identity is in the hash.
        Same implementation bytes, same image, same allowlist, same boundary — a new pid and a new
        container generation change nothing about the guarantee, and must not cost a
        re-certification. Only a materially different proof or boundary may."""
        es._docker = contained_docker()          # a fresh process, byte-identical implementation
        self.stamp()
        self.assertEqual(es.evaluate("ic")["state"], es.VERIFIED)
        # ...and nothing volatile was recorded to drift against.
        stamp = json.loads(pathlib.Path(os.environ["EGRESS_STAMP"]).read_text())
        for volatile in ("pid", "container_id", "started_at", "generation", "invocation_id"):
            self.assertNotIn(volatile, stamp,
                             f"{volatile!r} is in the stamp; a harmless restart would now force "
                             "re-certification for no change in the guarantee")

    def test_gateway_implementation_reads_the_announced_hash(self):
        es._docker = contained_docker(gateway_impl="a" * 64)
        self.assertEqual(es.gateway_implementation("gw"), "a" * 64)
        es._docker = contained_docker(gateway_impl=None)
        self.assertIsNone(es.gateway_implementation("gw"),
                          "a gateway that announced nothing must not read as agreeing")

    def test_current_start_epoch_chooses_B_after_historical_A_and_checkout_A(self):
        checkout_a = es._sha256_of(es._REPO / "deploy/egress/connect-proxy.py")
        current_b = "b" * 64
        base = contained_docker()

        def accumulated(*args):
            if args[0] == "logs":
                return 0, ("2026-08-27T11:00:00.000000000Z impl sha256=" + checkout_a + "\n"
                           + LOGGED + " impl sha256=" + current_b), ""
            return base(*args)

        es._docker = accumulated
        self.stamp()
        self.assertEqual(es.gateway_implementation("gw"), current_b)
        st = es.evaluate("ic")
        self.assertEqual(st["state"], es.RUNNING)
        self.assertIn("executing a different", " ".join(st["why"]))

    def test_topology_drift_and_unreadable_metadata_invalidate_certification(self):
        cases = {
            "runtime added": ({"ic": ["inner", "extra"]},
                              {"inner": True, "extra": True}, ["inner"]),
            "runtime replaced": ({"ic": ["other"]},
                                 {"other": True, "inner": True}, ["inner"]),
            "gateway added": ({"ic": ["inner"]},
                              {"inner": True, "edge": False}, ["inner", "edge"]),
            "shared removed": ({"ic": ["inner"]},
                               {"inner": True, "other": False}, ["other"]),
        }
        es._docker = contained_docker(); self.stamp()
        stamp = pathlib.Path(os.environ["EGRESS_STAMP"]).read_text()
        for label, (networks, flags, gateway_nets) in cases.items():
            with self.subTest(label):
                pathlib.Path(os.environ["EGRESS_STAMP"]).write_text(stamp)
                es._docker = fake_docker(networks, flags, True,
                                         gateway_networks=gateway_nets)
                self.assertNotEqual(es.evaluate("ic")["state"], es.VERIFIED)

        pathlib.Path(os.environ["EGRESS_STAMP"]).write_text(stamp)
        es._docker = fake_docker({"ic": ["inner"]}, {}, True,
                                 gateway_networks=["inner"])
        self.assertNotEqual(es.evaluate("ic")["state"], es.VERIFIED)

    def test_topology_ordering_only_is_normalized(self):
        es._docker = fake_docker({"ic": ["inner", "extra"]},
                                 {"inner": True, "extra": True}, True,
                                 gateway_networks=["inner", "extra"])
        self.stamp()
        es._docker = fake_docker({"ic": ["extra", "inner"]},
                                 {"inner": True, "extra": True}, True,
                                 gateway_networks=["extra", "inner"])
        self.assertEqual(es.evaluate("ic")["state"], es.VERIFIED)

    def test_same_named_recreated_network_invalidates_certification(self):
        es._docker = contained_docker(); self.stamp()
        es._docker = fake_docker({"ic": ["inner"]}, {"inner": True}, True,
                                 gateway_networks=["inner"],
                                 network_ids={"inner": "replacement-network-id"})
        self.assertNotEqual(es.evaluate("ic")["state"], es.VERIFIED)

    def test_an_unreadable_network_is_treated_as_routed(self):
        """Fail closed: a network whose internal flag cannot be read must not be assumed
        internal, or an inspection failure would silently upgrade the verdict."""
        es._docker = fake_docker({"ic": ["mystery"]}, {}, True)
        self.assertEqual(es.evaluate("ic")["state"], es.FAILED)

    def test_a_missing_container_is_BLOCKED_not_passing(self):
        es._docker = fake_docker({}, {}, True)
        st = es.evaluate("ic")
        self.assertEqual(st["state"], es.BLOCKED)

    def test_no_docker_is_BLOCKED(self):
        es._docker = lambda *a: (1, "", "not found")
        self.assertEqual(es.evaluate("ic")["state"], es.BLOCKED)


class FailClosed(Base):
    def test_17_losing_the_gateway_is_FAILED_and_explicitly_not_fail_open(self):
        """A dead gateway must never read as 'fine'. It is also not a return to open
        networking: the runtime stays internal-only, so the product breaks and the containment
        holds. The verdict says which of those happened."""
        es._docker = contained_docker(gateway_present=False)
        st = es.evaluate("ic")
        self.assertEqual(st["state"], es.FAILED)
        why = " ".join(st["why"])
        self.assertIn("gateway", why)
        self.assertIn("fail-CLOSED", why)
        self.assertNotIn("default route", why, "a dead gateway is not a routed network")

    def test_17b_a_gateway_that_exists_but_is_STOPPED_is_FAILED_like_a_missing_one(self):
        """EXISTING IS NOT RUNNING. `docker inspect` on an exited container exits 0 with its
        networks intact, so the presence check above passed for a gateway that had been
        `docker stop`ped or had OOMed out of its restart backoff — and `evaluate` returned
        VERIFIED for a boundary with no gateway behind it. 17 above fakes rc=1 (absent) and
        therefore could never have caught this."""
        es._docker = contained_docker(gateway_present=True, gateway_running=False)
        st = es.evaluate("ic")
        self.assertEqual(st["state"], es.FAILED)
        self.assertIn("gateway", " ".join(st["why"]))

    def test_a_stopped_runtime_is_BLOCKED_not_contained(self):
        """The stopped runtime's recorded networks are the ones it had when it stopped. Nothing
        about the live boundary was measured, so the verdict is 'could not measure', not a pass."""
        es._docker = contained_docker(running=False)
        st = es.evaluate("ic")
        self.assertEqual(st["state"], es.BLOCKED)
        self.assertIn("not running", " ".join(st["why"]))

    def test_a_deliberate_rollback_is_recorded_and_keeps_reporting(self):
        """Rolling back removes a control. It is allowed, and it must not be quiet."""
        es._docker = contained_docker()
        self.stamp()
        self.assertEqual(es.evaluate("ic")["state"], es.VERIFIED)
        pathlib.Path(os.environ["EGRESS_DEGRADED_MARK"]).write_text(json.dumps(
            {"state": "EGRESS_ROLLED_BACK", "at": "2026-08-24T00:00:00Z"}))
        st = es.evaluate("ic")
        self.assertEqual(st["state"], es.FAILED)
        self.assertTrue(st["deliberately_rolled_back"])
        self.assertIn("ROLLED BACK", " ".join(st["why"]))


class Stamp(Base):
    def test_the_stamp_holds_no_secret_and_is_0600(self):
        es._docker = contained_docker()
        doc = es.write_stamp("ic", IMAGE, "cloud-api.near.ai:443", 42)
        import stat
        p = pathlib.Path(os.environ["EGRESS_STAMP"])
        self.assertEqual(stat.S_IMODE(p.stat().st_mode), 0o600)
        blob = p.read_text().lower()
        for banned in ("token", "key", "secret", "bearer", "authorization"):
            self.assertNotIn(banned, blob, f"the stamp mentions {banned}")
        self.assertEqual(doc["container"], "ic")

    def test_only_a_passing_probe_writes_one(self):
        """Asserted at the call site rather than here: probe-egress.sh stamps only under
        `[ "$probe_rc" -eq 0 ]`. This pins that the guard is still there, because a stamp
        written by a failing probe would manufacture the exact confidence this avoids."""
        src = (pathlib.Path(__file__).resolve().parents[1]
               / "egress" / "probe-egress.sh").read_text()
        self.assertIn('if [ "$probe_rc" -eq 0 ]', src)
        stamp_at = src.index("write_stamp")
        guard_at = src.index('if [ "$probe_rc" -eq 0 ]')
        self.assertLess(guard_at, stamp_at, "the stamp is written outside the pass guard")


class DegradedMark(Base):
    """The rollback record's path, asserted at the WRITER. `degraded_mark_path` exists because a
    writer and a reader with independent copies of that rule once put the mark at one path and
    looked for it at another, and the boundary went on reporting VERIFIED through a deliberate
    rollback. The function fixes that only for as long as both sides keep asking it."""

    def control_src(self):
        return (pathlib.Path(__file__).resolve().parents[1]
                / "egress" / "egress-control.sh").read_text()

    def test_the_writer_asks_this_module_for_the_path(self):
        """Pinned on the PYTHON call, not on a name. egress-control.sh wraps the resolution in a
        shell function of the same name, so grepping `degraded_mark_path()` would go on matching
        the wrapper after its body had been replaced with a literal — which is the whole defect."""
        src = self.control_src()
        # assertTrue, not assertIn: a failing assertIn prints the entire script as the haystack,
        # which buries the one line that matters under a hundred that do not.
        self.assertTrue("es.degraded_mark_path()" in src,
                        "egress-control.sh no longer asks egress_status where the mark lives")
        self.assertTrue("es.write_degraded_mark(" in src,
                        "the rollback no longer records a degraded state through this module")

    def test_the_writer_derives_no_path_of_its_own(self):
        """The failure this guards is a SECOND derivation appearing beside the call, not the call
        disappearing — the two agree on the day it is written and diverge the day the default
        moves. A literal mark filename anywhere in the writer is that second derivation."""
        offenders = [f"egress-control.sh:{n}: {line.strip()}"
                     for n, line in enumerate(self.control_src().splitlines(), 1)
                     for lit in ("egress-degraded.json", ".agency/egress", "EGRESS_DEGRADED_MARK=")
                     if lit in line]
        self.assertEqual(offenders, [], "\n" + "\n".join(offenders) + "\nThe writer derives "
                         "the mark path itself. egress_status.degraded_mark_path() is the only "
                         "source; a second derivation agrees until the default moves.")

    def test_an_operator_override_reaches_both_sides(self):
        """Executed, not grepped. The writer resolves the path in a SEPARATE interpreter, so the
        two sides agree only if the shared function is driven by the environment rather than by
        anything process-local. Base has pointed EGRESS_DEGRADED_MARK at a temp file; a
        subprocess that answered the default instead is the divergence, one process removed."""
        got = subprocess.run(
            [sys.executable, "-c", "import sys; sys.path.insert(0, sys.argv[1]);"
                                   "import egress_status as es; print(es.degraded_mark_path())",
             str(pathlib.Path(__file__).resolve().parent)],
            capture_output=True, text=True, check=True).stdout.strip()
        self.assertEqual(got, str(es.degraded_mark_path()))


if __name__ == "__main__":
    unittest.main(verbosity=2)
