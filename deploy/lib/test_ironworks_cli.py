#!/usr/bin/env python3
"""The operator console's output contract. Offline, stdlib only, no live services.

Run: python3 deploy/lib/test_ironworks_cli.py

A console that prints a tenant's credentials while reporting on that tenant's health would be
worse than no console — an operator runs these commands in shared terminals, pastes them into
tickets, and pipes `--json` into artifacts. So the properties pinned here are the ones that
make the tool safe to use in those places, plus the exit codes anything scripting it depends
on.

NOTHING HERE TOUCHES A NETWORK, in either of the two shapes it uses. `ConsoleTest` runs the
console as a subprocess with `--offline`, where every LIVE check is skipped. `LiveCheckTest`
imports it and hands the LIVE checks a stubbed transport — because the defects those checks
can have are all in what they conclude from a response, and a verdict that only exists when a
service is up is a verdict nothing pins.
"""
import contextlib
import importlib.machinery
import importlib.util
import io
import json
import os
import pathlib
import sqlite3
import subprocess
import sys
import tempfile
import types
import unittest
import urllib.error
from unittest import mock

ROOT = pathlib.Path(__file__).resolve().parents[2]
CLI = ROOT / "deploy" / "ironworks"


def load_console():
    """The console as a module. Loaded by path because the entry point is extensionless —
    `import` cannot see it, and renaming it would change the operator's command."""
    loader = importlib.machinery.SourceFileLoader("ironworks_console", str(CLI))
    spec = importlib.util.spec_from_file_location("ironworks_console", str(CLI), loader=loader)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module

# Distinctive enough that a substring search cannot pass by accident.
IC_TOKEN_A = "MEMBERTOKEN-aaaa-1111-should-never-be-printed"
AC_TOKEN_A = "ORGTOKEN-aaaa-2222-should-never-be-printed"
IC_TOKEN_B = "MEMBERTOKEN-bbbb-3333-should-never-be-printed"
AC_TOKEN_B = "ORGTOKEN-bbbb-4444-should-never-be-printed"
SECRETS = (IC_TOKEN_A, AC_TOKEN_A, IC_TOKEN_B, AC_TOKEN_B)

GUIDANCE = """<!-- client-guidance v1 slug: {slug} -->
<!-- SYNTHETIC GUIDANCE — offline test fixture, not a real organization. -->
# Organization guidance — {name}
## Company & offer
{name} keeps a book of invented accounts used only to exercise the operator console.
## Target customer
Nobody. Every record is synthetic.
## Qualification criteria
- A stated problem in the record
- A named person who is still there
## Disqualification criteria
- A do-not-contact flag on the account
## Account stages
new -> reviewed -> decided. Recommend only these, or continue discovery.
## Supported evidence sources
The loaded account book and what the team states in chat.
## Desired decisions
Which accounts need attention this week, and what to ask next.
## Prohibited claims & actions
Never contact anyone. Read-only always.
"""


def write_registry(d, tenants):
    for slug, (name, ic, ac, gid, service) in tenants.items():
        (d / f"{slug}.guidance.md").write_text(GUIDANCE.format(slug=slug, name=name))
        lines = [f"CLIENT_SLUG={slug}", f"CLIENT_NAME={name}",
                 f"ACCOUNT_TOKEN={ac}", f"IRONCLAW_TOKEN={ic}"]
        if service:
            lines.append(f"SERVICE={service}")
        if gid:
            lines.append(f"TELEGRAM_GROUP_ID={gid}")
        (d / f"{slug}.env").write_text("\n".join(lines) + "\n")


class ConsoleTest(unittest.TestCase):
    maxDiff = None

    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.tmp = pathlib.Path(self._tmp.name)
        self.registry = self.tmp / "clients"
        self.registry.mkdir()
        write_registry(self.registry, {
            "acme": ("Acme Corp", IC_TOKEN_A, AC_TOKEN_A, "-100900001", "account-analysis"),
            "multiagency": ("MultiAgency", IC_TOKEN_B, AC_TOKEN_B, "-100900002",
                            "relationship-intelligence"),
        })
        # An internal tenant needs its guidance bound to the internal service, exactly like any
        # other tenant — the console must be able to report on it with no special case.
        g = self.registry / "multiagency.guidance.md"
        g.write_text(g.read_text().replace(
            "<!-- client-guidance v1 slug: multiagency -->",
            "<!-- client-guidance v1 slug: multiagency service: relationship-intelligence -->"))
        self.state = self.tmp / "bridge-threads.json"
        self.state.write_text(json.dumps({
            "-100900001": {"prev": "resp_1", "supplied": {"A-1": "2026-08-01T00:00:00+00:00"},
                           "ever_supplied": True, "last_turn_at": "2026-08-20T09:00:00+00:00",
                           "orphans": {"GHOST-9": ["2026-08-01T00:00:00+00:00", 1]}},
        }))
        self.ledger = self.tmp / "residual.json"

    def tearDown(self):
        self._tmp.cleanup()

    def run_cli(self, *args, ledger=None):
        env = {**os.environ,
               "CLIENTS_DIR": str(self.registry),
               "BRIDGE_STATE": str(self.state),
               "RESIDUAL_LEDGER": str(ledger or self.ledger),
               # Never inherit the operator's real egress stamp: a host that happens to have a
               # valid one would make these assertions pass for the wrong reason.
               "EGRESS_STAMP": str(self.tmp / "egress-stamp.json"),
               "EGRESS_DEGRADED_MARK": str(self.tmp / "egress-degraded.json"),
               "AGENCY_DIR": str(self.tmp),
               "PERSONA_ROOT": str(ROOT),
               # Point at a port nothing serves, so an accidentally-unskipped LIVE check
               # cannot reach a real instance from a unit test.
               "IRONCLAW_API": "http://127.0.0.1:9",
               "ACCOUNT_BASE": "http://127.0.0.1:9"}
        env.pop("MODEL", None)
        return subprocess.run([sys.executable, str(CLI), *args],
                              capture_output=True, text=True, env=env, cwd=ROOT, timeout=120)

    def run_json(self, *args, ledger=None):
        """The console's JSON artifact for one command. `--json` is prepended here because the
        output contract says JSON goes to stdout with nothing else on it — a caller that forgets
        the flag gets a JSONDecodeError rather than a useful failure."""
        return json.loads(self.run_cli("--offline", "--json", *args, ledger=ledger).stdout)

    def check_by_id(self, doc, check_id):
        """One check out of a report, by id, failing with the ids that WERE present.

        `next(c for c in doc["checks"] if c["id"] == ...)` was written out nine times, and its
        failure mode is a bare StopIteration naming nothing — least useful exactly when a check
        has been renamed or has stopped being emitted, which is the reason it would fail."""
        for c in doc["checks"]:
            if c["id"] == check_id:
                return c
        raise AssertionError(
            f"no check {check_id!r} in this report; present: {[c['id'] for c in doc['checks']]}")

    # ── the property that matters most ────────────────────────────────────────────────

    def test_no_credential_appears_in_any_output(self):
        """Every command, both formats. An operator pastes this into tickets."""
        for args in (["--offline", "doctor"],
                     ["--offline", "--json", "doctor"],
                     ["--offline", "tenants", "status"],
                     ["--offline", "--json", "tenants", "status"],
                     ["--offline", "tenant", "inspect", "acme"],
                     ["--offline", "--json", "tenant", "inspect", "multiagency"],
                     ["--offline", "service", "validate"],
                     ["--offline", "account-db", "migration-status"],
                     ["--offline", "--json", "release", "verify", "--offline-only"],
                     ["--offline", "tenant", "reset-thread", "acme"],
                     ["--offline", "bridge", "redeliver", "7"]):
            r = self.run_cli(*args)
            blob = r.stdout + r.stderr
            for secret in SECRETS:
                self.assertNotIn(secret, blob, f"{' '.join(args)} printed a credential")

    def test_a_registry_error_message_does_not_echo_the_credential(self):
        """The riskiest path: load_clients raises with the offending file's contents in reach.
        Two tenants sharing a member token IS the same identity, so it fails closed — and the
        message must name the tenants, not the token."""
        write_registry(self.registry, {
            "twin": ("Twin", IC_TOKEN_A, AC_TOKEN_B, "-100900003", None)})
        r = self.run_cli("--offline", "doctor")
        blob = r.stdout + r.stderr
        self.assertIn("registry.load", blob)
        for secret in SECRETS:
            self.assertNotIn(secret, blob)
        self.assertNotEqual(r.returncode, 0, "a cross-wired registry must not report success")

    # ── exit codes ────────────────────────────────────────────────────────────────────

    def test_no_status_path_opens_the_bridge_store_for_writing(self):
        """A STATUS CHECK MAY NOT MIGRATE. Every read path here must pass `migrate=False`;
        `tenant reset-thread` is the one legitimate writer, because an operator asked for it.

        Source-level because the defect was invisible at the console's own output: `--offline
        doctor` upgraded the operator's live database and wrote a backup beside it while printing
        nothing about either, and the `except Exception: return {}` around the thread view is
        what swallowed the evidence."""
        import re
        console = (ROOT / "deploy" / "ironworks").read_text()
        # The two commands an operator invokes ON PURPOSE to change bridge state. Everything
        # else — doctor, tenants status, tenant inspect, bridge status — only looks.
        permitted = ("cmd_tenant_reset_thread", "cmd_bridge_redeliver")
        enclosing, offenders, seen = None, [], 0
        for line in console.splitlines():
            m = re.match(r"def (\w+)\(", line)
            if m:
                enclosing = m.group(1)
            stripped = line.strip()
            if "BridgeState(" not in stripped or stripped.startswith("#"):
                continue
            seen += 1
            if "migrate=False" in stripped or enclosing in permitted:
                continue
            offenders.append(f"{enclosing}: {stripped}")
        self.assertGreaterEqual(seen, 3, "this guard is looking in the wrong place")
        self.assertEqual(offenders, [],
                         "these console paths open the bridge store for WRITING, so running them "
                         "migrates the operator's live database as a side effect: " +
                         "; ".join(offenders))

    def test_exit_code_is_2_when_a_check_fails(self):
        r = self.run_cli("--offline", "tenant", "inspect", "nope")
        self.assertEqual(r.returncode, 2, r.stdout)

    def test_exit_code_is_64_on_usage_error(self):
        self.assertEqual(self.run_cli("--offline").returncode, 64)
        self.assertEqual(self.run_cli("--offline", "tenant").returncode, 64)

    def test_a_clean_offline_service_validate_exits_0(self):
        r = self.run_cli("--offline", "service", "validate")
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)

    def test_offline_migration_status_is_explicitly_skipped(self):
        """The verdict is the subject here: SKIPPED, never PASS.

        The exit code was 0 and is now 3. Deliberate: this report's only row is SKIPPED, so
        nothing was evaluated, and 0 is a verdict a script reads as healthy — see
        `test_a_report_that_evaluated_nothing_does_not_exit_zero`. 3 is the console's documented
        code for exactly this ("BLOCKED means a guarantee was NOT evaluated"), and no script or
        document drives this command: only `doctor --offline` is scripted, and it still exits 0
        because it evaluates its CONFIG half."""
        r = self.run_cli("--offline", "--json", "account-db", "migration-status")
        doc = json.loads(r.stdout)
        self.assertEqual(r.returncode, 3, doc)
        self.assertEqual(self.check_by_id(doc, "account.ready")["verdict"], "SKIPPED")

    def test_outstanding_residual_authority_fails_the_doctor(self):
        """The ledger is only useful if something reads it and refuses to say 'fine'."""
        self.ledger.write_text(json.dumps({
            "gone": {"slug": "gone", "uid": "u-1", "deleted_at": "2026-08-01T00:00:00+00:00",
                     "expires_at": "2099-01-01T00:00:00+00:00",
                     "expires_at_epoch": 4070908800, "session_lifetime_days": 365}}))
        # run_cli, not run_json: this test asserts on the EXIT CODE as well as the report, and
        # the exit code is the half a script depends on.
        r = self.run_cli("--offline", "--json", "doctor")
        check = self.check_by_id(json.loads(r.stdout), "revocation.residual")
        self.assertEqual(check["verdict"], "FAIL", check)
        self.assertEqual(r.returncode, 2)

    def test_an_expired_residual_entry_does_not_fail_the_doctor(self):
        self.ledger.write_text(json.dumps({
            "old": {"slug": "old", "uid": "u-2", "deleted_at": "2020-01-01T00:00:00+00:00",
                    "expires_at": "2021-01-01T00:00:00+00:00",
                    "expires_at_epoch": 1609459200, "session_lifetime_days": 365}}))
        doc = self.run_json("doctor")
        check = self.check_by_id(doc, "revocation.residual")
        self.assertEqual(check["verdict"], "PASS", check)

    # ── the config / live distinction ─────────────────────────────────────────────────

    def test_every_check_declares_config_or_live(self):
        doc = self.run_json("doctor")
        self.assertTrue(doc["checks"])
        for c in doc["checks"]:
            self.assertIn(c["kind"], ("config", "live"), c)
            self.assertIn(c["verdict"], ("PASS", "FAIL", "BLOCKED", "SKIPPED"), c)

    def test_offline_skips_live_checks_rather_than_passing_them(self):
        """A skipped check must never be counted as evidence. This is the difference between
        an honest artifact and a green one.

        ASSERTED FOR EVERY COMMAND THAT EMITS A LIVE ROW, not just `doctor`. Pinned to `doctor`
        alone, this passed while `--offline bridge status` went on evaluating a LIVE-kind
        verdict — the exact shape the flag exists to make impossible — and while
        `--offline bridge redeliver` built a real Telegram client and sent a message into a
        client group. A contract enforced at one call site is a convention, not a contract."""
        commands = (("doctor",), ("tenants", "status"), ("tenant", "inspect", "acme"),
                    ("bridge", "status"), ("account-db", "migration-status"))
        saw_live = False
        for cmd in commands:
            doc = self.run_json(*cmd)
            live = [c for c in doc["checks"] if c["kind"] == "live"]
            saw_live = saw_live or bool(live)
            self.assertTrue(all(c["verdict"] == "SKIPPED" for c in live),
                            (cmd, [c for c in live if c["verdict"] != "SKIPPED"]))
        self.assertTrue(saw_live, "no live checks at all — the distinction would be vacuous")

    def test_a_report_that_evaluated_nothing_does_not_exit_zero(self):
        """SECURITY.md § delivery: "Use `./deploy/ironworks bridge status`; exit `0` is healthy,
        `2` unhealthy, and `3` unevaluated." An all-SKIPPED report exiting 0 therefore says
        HEALTHY about a bridge nothing looked at — the same green-means-nothing shape the
        promotion gate had. `multi/verify/common.Checks.finish` already holds this doctrine for
        the proof suites; this puts it in the console.

        A report with ANY evaluated row is unaffected, which is why `--offline doctor` — nine
        passing CONFIG rows beside five skipped LIVE ones — must still exit 0."""
        skipped_only = self.run_cli("--offline", "--json", "bridge", "status")
        doc = json.loads(skipped_only.stdout)
        self.assertTrue(doc["checks"], "nothing to reason about")
        self.assertTrue(all(c["verdict"] == "SKIPPED" for c in doc["checks"]), doc["checks"])
        self.assertEqual(skipped_only.returncode, 3,
                         "a report that measured nothing exited 0, which reads as healthy")

        mixed = self.run_cli("--offline", "--json", "doctor")
        mixed_doc = json.loads(mixed.stdout)
        self.assertTrue(any(c["verdict"] == "PASS" for c in mixed_doc["checks"]))
        self.assertTrue(any(c["verdict"] == "SKIPPED" for c in mixed_doc["checks"]))
        self.assertEqual(mixed.returncode, 0,
                         "skipping the LIVE half must not make an evaluated report unevaluated")

    def test_offline_refuses_the_commands_whose_whole_effect_is_live(self):
        """`bridge redeliver` sends a real message into a client group. There is no offline half
        of it to run, so honouring `--offline` means refusing — it used to build a live
        `_Telegram()` and post to the tenant regardless."""
        r = self.run_cli("--offline", "--json", "bridge", "redeliver", "5", "--confirm", "5")
        doc = json.loads(r.stdout)
        check = self.check_by_id(doc, "delivery.redelivered")
        self.assertEqual(check["verdict"], "SKIPPED", check)
        self.assertIn("--offline", check["detail"])

    def test_json_test_aggregates_one_stubbed_gate_without_recursive_recollection(self):
        """`--json` means stdout is JSON and nothing else. `cmd_test` discarded the flag and
        printed run-quality's human report straight through it, so `--json test | jq` failed.

        Stub the ONE subprocess boundary rather than launching the aggregate gate from inside
        the aggregate gate's own pytest process. Calling `run_cli("--json", "test")` here
        recursively recollects this test forever. The real console aggregation code still runs;
        only the already-independently-tested child gate is replaced with its process result."""
        console = load_console()
        child = subprocess.CompletedProcess(
            [sys.executable, str(ROOT / "deploy" / "run-quality.py")], 0,
            stdout="17 passed · 0 FAILED · 0 BLOCKED\nALL QUALITY GATES PASS\n",
            stderr="child diagnostic\n")
        stdout, stderr = io.StringIO(), io.StringIO()
        with mock.patch.object(console.subprocess, "run", return_value=child) as run_gate:
            with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
                code = console.cmd_test(types.SimpleNamespace(json=True))

        self.assertEqual(code, 0)
        doc = json.loads(stdout.getvalue())  # raises if the human report leaked onto stdout
        self.assertEqual([c["id"] for c in doc["checks"]], ["gate.quality"])
        self.assertEqual(doc["quality_gate"]["exit_code"], 0)
        self.assertEqual(doc["checks"][0]["verdict"], "PASS")
        self.assertIn("ALL QUALITY GATES PASS", stderr.getvalue())
        self.assertIn("child diagnostic", stderr.getvalue())
        run_gate.assert_called_once_with(
            [sys.executable, str(ROOT / "deploy" / "run-quality.py")], cwd=ROOT,
            capture_output=True, text=True)

    def test_json_is_the_only_thing_on_stdout_in_json_mode(self):
        """So `... --json | jq` works without a filter, and an artifact is a file, not a log."""
        for args in (["--offline", "--json", "doctor"],
                     ["--offline", "--json", "tenants", "status"],
                     ["--offline", "--json", "tenant", "inspect", "acme"],
                     ["--offline", "--json", "service", "validate"]):
            r = self.run_cli(*args)
            json.loads(r.stdout)     # raises if anything else was printed

    # ── operator questions the console exists to answer ───────────────────────────────

    def test_it_reports_service_and_version_per_tenant(self):
        doc = self.run_json("tenants", "status")
        by_slug = {t["slug"]: t for t in doc["tenants"]}
        self.assertEqual(by_slug["acme"]["service"], "account-analysis@1")
        self.assertEqual(by_slug["multiagency"]["service"], "relationship-intelligence@1")
        for t in by_slug.values():
            self.assertTrue(t["persona_sha"], t)

    def test_the_internal_tenant_is_reported_like_any_other(self):
        """No special case, no extra field, no missing check: the console is one of the places
        'MultiAgency is a tenant like the others' has to be visibly true."""
        internal = json.loads(
            self.run_cli("--offline", "--json", "tenant", "inspect", "multiagency").stdout)
        external = json.loads(
            self.run_cli("--offline", "--json", "tenant", "inspect", "acme").stdout)
        self.assertEqual(sorted(internal["tenant"]), sorted(external["tenant"]))
        self.assertEqual([c["id"] for c in internal["checks"]],
                         [c["id"] for c in external["checks"]])

    def test_it_surfaces_a_catalog_orphan(self):
        """A catalogued account whose context 404s is a store inconsistency the client never
        sees. If the console does not report it, nobody learns about it."""
        doc = self.run_json("tenant", "inspect", "acme")
        self.assertEqual(doc["tenant"]["catalog_orphans"], ["GHOST-9"])
        check = self.check_by_id(doc, "tenant.catalog_consistent")
        self.assertEqual(check["verdict"], "FAIL")

    def test_it_reports_last_turn_and_distinguishes_never(self):
        doc = self.run_json("tenants", "status")
        by_slug = {t["slug"]: t for t in doc["tenants"]}
        self.assertEqual(by_slug["acme"]["last_turn_at"], "2026-08-20T09:00:00+00:00")
        self.assertIsNone(by_slug["multiagency"]["last_turn_at"],
                          "a tenant that has never run a turn must read as unknown, not as 0")

    def test_an_unrouted_tenant_is_a_failure_not_a_footnote(self):
        write_registry(self.registry, {
            "orphan": ("Orphan", "IC-ORPHAN-TOKEN", "AC-ORPHAN-TOKEN", "", None)})
        doc = self.run_json("tenants", "status")
        check = self.check_by_id(doc, "tenant.orphan.routing")
        self.assertEqual(check["verdict"], "FAIL")

    def test_reset_thread_requires_confirmation_and_stopped_bridge_then_preserves_delivery(self):
        preview = self.run_cli("--offline", "--json", "tenant", "reset-thread", "acme")
        doc = json.loads(preview.stdout)
        self.assertEqual(preview.returncode, 2, doc)
        self.assertEqual(doc["thread_reset"]["stored_identity"]["service"], None)
        self.assertEqual(doc["thread_reset"]["intended_identity"]["service"],
                         "account-analysis")
        confirm = self.check_by_id(doc, "thread.confirmed")
        self.assertIn("--confirm acme", confirm["detail"])

        db_path = self.state.with_suffix(".db")
        db = sqlite3.connect(str(db_path))
        # A live recorded pid is a hard stop. Use this test process: the CLI child can prove
        # its parent is alive without starting a bridge or depending on process-name parsing.
        db.execute("INSERT INTO meta(key,value) VALUES('pid',?) "
                   "ON CONFLICT(key) DO UPDATE SET value=excluded.value", (str(os.getpid()),))
        db.commit(); db.close()
        running = self.run_cli("--offline", "--json", "tenant", "reset-thread", "acme",
                               "--confirm", "acme")
        running_doc = json.loads(running.stdout)
        self.assertEqual(running.returncode, 2, running_doc)
        self.assertEqual(next(c for c in running_doc["checks"]
                              if c["id"] == "bridge.stopped")["verdict"], "FAIL")

        db = sqlite3.connect(str(db_path))
        self.assertIsNotNone(db.execute("SELECT gid FROM threads WHERE gid=?",
                                        ("-100900001",)).fetchone(),
                             "running-bridge guard reset the thread anyway")
        db.execute("UPDATE meta SET value=NULL WHERE key='pid'")
        db.execute("INSERT INTO updates(update_id,gid,state) VALUES(?,?,?)",
                   (91, "-100900001", "IGNORED"))
        db.execute("INSERT INTO meta(key,value) VALUES('cursor','92') "
                   "ON CONFLICT(key) DO UPDATE SET value=excluded.value")
        db.commit(); db.close()

        done = self.run_cli("--offline", "--json", "tenant", "reset-thread", "acme",
                            "--confirm", "acme")
        done_doc = json.loads(done.stdout)
        self.assertEqual(done.returncode, 0, done_doc)
        self.assertEqual(done_doc["thread_reset"]["removed_rows"], 1)
        self.assertEqual(done_doc["thread_reset"]["delivery_journal"], "preserved")

        db = sqlite3.connect(str(db_path))
        self.assertIsNone(db.execute("SELECT gid FROM threads WHERE gid=?",
                                     ("-100900001",)).fetchone())
        self.assertIsNotNone(db.execute("SELECT update_id FROM updates WHERE update_id=91").fetchone())
        self.assertEqual(db.execute("SELECT value FROM meta WHERE key='cursor'").fetchone()[0], "92")
        db.close()

    def test_delivery_redelivery_requires_exact_confirmation_before_live_work(self):
        r = self.run_cli("--offline", "--json", "bridge", "redeliver", "71")
        doc = json.loads(r.stdout)
        self.assertEqual(r.returncode, 2, doc)
        check = self.check_by_id(doc, "delivery.confirmed")
        self.assertIn("--confirm 71", check["detail"])

    def test_the_egress_check_is_live_and_never_silently_passes_offline(self):
        """Containment became a LIVE check when it started reading the running container rather
        than the compose file — so `--offline` must SKIP it, not pass it. A boundary reported
        present because a file exists is the failure the whole state machine exists to avoid."""
        doc = self.run_json("doctor")
        check = self.check_by_id(doc, "egress.network")
        self.assertEqual(check["kind"], "live")
        self.assertEqual(check["verdict"], "SKIPPED", check)
        # ...and when it IS evaluated, an absent boundary costs a non-zero exit. (The live
        # verdict itself is asserted in test_18 below.)
        live = json.loads(self.run_cli("--json", "doctor").stdout)
        lcheck = next(c for c in live["checks"] if c["id"] == "egress.network")
        self.assertNotEqual(lcheck["verdict"], "SKIPPED")

    def test_18_doctor_reports_containment_accurately(self):
        """The console must never call an unproved boundary present. On this host the runtime
        is on a routed network, so the only correct answers are FAIL (evaluated, absent) or
        BLOCKED (not evaluable) — never PASS."""
        r = self.run_cli("--json", "doctor")
        doc = json.loads(r.stdout)
        check = self.check_by_id(doc, "egress.network")
        self.assertIn(check["verdict"], ("FAIL", "BLOCKED"),
                      f"containment reported as {check['verdict']} without proof: {check}")
        self.assertNotEqual(r.returncode, 0)

    def test_19_release_verify_refuses_promotion_without_containment(self):
        """A release artifact that reports a green test suite beside an unproved network
        boundary invites exactly the wrong reading. Promotion is a decision, not a row."""
        r = self.run_cli("--json", "release", "verify", "--offline-only")
        doc = json.loads(r.stdout)
        gate = self.check_by_id(doc, "release.promotable")
        self.assertEqual(gate["verdict"], "FAIL", gate)
        self.assertIn("egress containment", gate["detail"])
        self.assertFalse(doc["promotable"])
        self.assertNotEqual(r.returncode, 0)

    def test_19b_an_unrun_gate_refuses_promotion_even_with_containment_VERIFIED(self):
        """THE HOST THE TEST ABOVE CANNOT REACH. On a laptop egress is never VERIFIED, so the
        assertion above is satisfied by the egress clause alone and says nothing about the gate
        rows. On the serve host — the only place egress IS VERIFIED, and so the only place this
        verdict can come out true — `--offline-only` marked every gate SKIPPED and the rule only
        looked for FAIL rows, so the artifact certified promotion having run zero tests.

        Driven against `promotion_gate` directly, with the boundary held at VERIFIED, because
        that is the state a subprocess on this machine cannot produce."""
        console = load_console()

        def rows(*specs):
            return [{"id": i, "verdict": v, "kind": k} for i, v, k in specs]

        skipped = rows(("gate.seam.registry", "SKIPPED", "config"),
                       ("gate.lib.lifecycle", "SKIPPED", "config"))
        promotable, why = console.promotion_gate(skipped, "VERIFIED")
        self.assertFalse(promotable, "every gate SKIPPED and the release reported promotable")
        self.assertIn("gate.seam.registry", why)

        # A gate BLOCKED by a missing interpreter or a moved directory (`cannot run: ...`) is
        # the same defect in a different verdict, and keying on FAIL/SKIPPED alone missed it.
        passing = rows(("gate.seam.registry", "PASS", "config"),
                       ("gate.lib.lifecycle", "PASS", "config"))
        self.assertEqual(console.promotion_gate(passing, "VERIFIED"), (True, ""))
        for bad in ("FAIL", "BLOCKED", "SKIPPED"):
            verdict, why = console.promotion_gate(
                passing + rows(("gate.accountdb.migrations", bad, "config")), "VERIFIED")
            self.assertFalse(verdict, f"a {bad} gate row was promotable")
            self.assertIn("gate.accountdb.migrations", why)

        # A LIVE row is BLOCKED here BY CONSTRUCTION — named, not required. Requiring it would
        # make the verdict unreachable on every host, which is a different way of being useless.
        self.assertTrue(console.promotion_gate(
            passing + rows(("live.isolation", "BLOCKED", "live")), "VERIFIED")[0])

        # No gate suite ran at all: the discovery globs matched nothing. That must not read as
        # "nothing failed".
        self.assertFalse(console.promotion_gate(
            rows(("registry.load", "PASS", "config")), "VERIFIED")[0])

        self.assertFalse(console.promotion_gate(passing, "RUNNING")[0])

    def test_the_documented_post_subcommand_flag_forms_actually_parse(self):
        """EVERY FORM THE DOCS USE, RUN. `--json` and `--offline` lived on the parent parser
        only, so `tenants status --json` — the form in docs/INCIDENT_RESPONSE.md:109,
        README.md:81, deploy/README.md:9 and deploy/UPGRADE.md:340, and the one
        egress-control.sh reaches through `exec … egress status "$@"` — was an argparse usage
        error: nothing on stdout, exit 2. That collides with this console's own EXIT_FAILED,
        so a script wrapping it read a parse error as a degraded tenant, and an incident
        responder following the runbook got no output at all.

        Asserted by PARSING THE STDOUT, because exit 2 is a legitimate verdict here and cannot
        distinguish the two on its own — which is precisely why the defect survived."""
        forms = (("tenants", "status", "--json"),
                 ("--json", "tenants", "status"),
                 ("egress", "status", "--json"),
                 ("service", "validate", "--json"),
                 ("doctor", "--offline", "--json"),
                 ("bridge", "status", "--json"),
                 ("release", "verify", "--offline-only", "--json"))
        for form in forms:
            r = self.run_cli(*form)
            self.assertNotIn("usage: ironworks", r.stderr,
                             f"{' '.join(form)} is a usage error, not a command")
            try:
                doc = json.loads(r.stdout)
            except json.JSONDecodeError as e:
                raise AssertionError(
                    f"{' '.join(form)} produced no JSON artifact ({e}); stdout was "
                    f"{r.stdout[:120]!r}") from e
            self.assertIn("checks", doc, form)

    def test_egress_status_never_passes_without_a_verification_stamp(self):
        r = self.run_cli("--json", "egress", "status")
        doc = json.loads(r.stdout)
        st = doc.get("egress", {}).get("state")
        self.assertIn(st, ("FAILED", "BLOCKED", "RUNNING"), doc.get("egress"))
        self.assertNotIn(st, ("VERIFIED",), "VERIFIED without a stamp on this host")

    def test_release_verify_names_what_it_could_not_run(self):
        """A readiness artifact whose blocked items are invisible is a green artifact."""
        doc = self.run_json("release", "verify",
                                      "--offline-only")
        blocked = {c["id"] for c in doc["checks"] if c["verdict"] == "BLOCKED"}
        for required in ("live.isolation", "live.egress", "live.revocation", "live.eval"):
            self.assertIn(required, blocked)


class LiveCheckTest(unittest.TestCase):
    """The LIVE checks, driven with a stubbed transport.

    A check that only runs when a service is answering is a check nobody exercises, and all
    three defects fixed here lived in that gap: an unmapped status read as accepted, a health
    verdict that turned on the runtime's JSON whitespace, and an error body dropped on the
    floor. So the transport is replaced and the CONCLUSION is what gets asserted."""

    maxDiff = None

    @classmethod
    def setUpClass(cls):
        cls.console = load_console()

    def setUp(self):
        self.rep = self.console.Report("test")
        self._real_http = self.console.http
        self.addCleanup(setattr, self.console, "http", self._real_http)

    def stub_http(self, responses, default=(0, None)):
        """Answer by URL suffix. `default` covers the probes a check makes in passing (the
        `/auth/logout` route sniff), so a test states only the response it is about."""
        def fake(url, **_kw):
            for suffix, response in responses.items():
                if url.endswith(suffix):
                    return response
            return default
        self.console.http = fake

    def check(self, cid):
        for c in self.rep.checks:
            if c["id"] == cid:
                return c
        raise AssertionError(
            f"no check {cid!r}; present: {[c['id'] for c in self.rep.checks]}")

    # ── 1. an unmapped status must not read as accepted ───────────────────────────────

    def test_the_auth_probe_reads_only_the_codes_that_mean_something(self):
        """401/403 refuse, 2xx and 404 got past auth, 0 is silence. Everything else is an
        answer about the gateway, not about the token."""
        verdict = self.console.auth_verdict
        self.assertEqual(verdict(0), "unreachable")
        for code in (401, 403):
            self.assertEqual(verdict(code), "rejected", code)
        for code in (200, 204, 404):
            self.assertEqual(verdict(code), "accepted", code)
        for code in (400, 408, 429, 500, 502, 503, 504):
            self.assertEqual(verdict(code), "undetermined", code)

    def test_a_gateway_error_on_the_auth_probe_blocks_and_never_passes(self):
        """The defect this replaces: a 500, a 502 from a proxy in front of the instance, or a
        429 printed a green `member token accepted` line for a tenant whose auth was never
        established. BLOCKED exits 3, so an unevaluated guarantee still costs an exit code."""
        for code in (429, 500, 502, 503):
            rep = self.console.Report("test")
            row = {"auth": self.console.auth_verdict(code), "auth_status": code}
            self.console.report_auth(rep, "tenant.acme.auth", "acme: member token accepted", row)
            self.assertEqual(rep.checks[0]["verdict"], "BLOCKED", rep.checks)
            self.assertIn(str(code), rep.checks[0]["detail"])
            self.assertEqual(rep.exit_code, self.console.EXIT_BLOCKED)

    def test_both_tenant_views_answer_the_auth_probe_the_same_way(self):
        """`tenants status` and `tenant inspect` read one probe. A status that PASSes on one
        screen and BLOCKs on the other is a disagreement settled by believing the greener."""
        for code in (0, 200, 401, 403, 404, 429, 500):
            row = {"auth": self.console.auth_verdict(code), "auth_status": code}
            status_rep, inspect_rep = self.console.Report("a"), self.console.Report("b")
            self.console.report_auth(status_rep, "tenant.acme.auth", "acme: accepted", row)
            self.console.report_auth(inspect_rep, "tenant.auth", "the token authenticates", row)
            self.assertEqual(status_rep.checks[0]["verdict"],
                             inspect_rep.checks[0]["verdict"], code)

    def test_a_rejected_token_still_fails_and_names_the_status(self):
        row = {"auth": self.console.auth_verdict(401), "auth_status": 401}
        self.console.report_auth(self.rep, "tenant.auth", "the token authenticates", row)
        self.assertEqual(self.check("tenant.auth")["verdict"], "FAIL")
        self.assertIn("401", self.check("tenant.auth")["detail"])

    # ── 2. health is parsed, not substring-matched ────────────────────────────────────

    def test_instance_health_does_not_turn_on_json_whitespace(self):
        """`'"status":"healthy"' in body` made a reformat upstream take a healthy fleet red."""
        for body in ('{"status":"healthy"}', '{"status": "healthy"}',
                     '{\n  "status" : "healthy",\n  "version": "x"\n}'):
            rep = self.console.Report("test")
            self.stub_http({"/api/health": (200, body)})
            self.console.check_instance(rep)
            self.assertEqual(next(c for c in rep.checks if c["id"] == "instance.health")
                             ["verdict"], "PASS", body)

    def test_instance_health_reads_the_field_and_not_the_text_around_it(self):
        """A substring match cannot tell a field from a sentence that mentions one."""
        self.stub_http({"/api/health": (
            200, '{"note": "we want status:healthy here", "status": "degraded"}')})
        self.console.check_instance(self.rep)
        check = self.check("instance.health")
        self.assertEqual(check["verdict"], "FAIL")
        self.assertIn("degraded", check["detail"])

    def test_an_unparseable_health_body_fails_and_says_why(self):
        self.stub_http({"/api/health": (200, "<html>502 Bad Gateway</html>")})
        self.console.check_instance(self.rep)
        check = self.check("instance.health")
        self.assertEqual(check["verdict"], "FAIL")
        self.assertIn("not JSON", check["detail"])
        self.assertIn("Bad Gateway", check["detail"], "the body an operator needs is gone")

    def test_account_health_does_not_turn_on_json_whitespace(self):
        for body in ('{"ok":true}', '{"ok": true}', '{ "ok" :\n true }'):
            rep = self.console.Report("test")
            self.stub_http({"/health": (200, body)}, default=(0, None))
            self.console.check_account_service(rep)
            self.assertEqual(next(c for c in rep.checks if c["id"] == "account.health")
                             ["verdict"], "PASS", body)

    def test_account_health_requires_the_value_true_not_the_text_true(self):
        self.stub_http({"/health": (200, '{"ok": "true"}')})
        self.console.check_account_service(self.rep)
        self.assertEqual(self.check("account.health")["verdict"], "FAIL")

    # ── 2b. the leak check: an allowlist, not a keyword list ──────────────────────────

    def test_a_clean_health_body_is_not_reported_as_a_leak(self):
        leaked, why = self.console.account_health_leak({"ok": True})
        self.assertFalse(leaked, why)

    def test_the_services_own_error_body_passes_its_own_allowlist(self):
        """The delegation, pinned from both ends: the allowlist is built from `safe_error`,
        so `safe_error`'s output must satisfy it. If the service changes what it says, this
        fails here rather than turning into a mystery FAIL on a live box."""
        guards = self.console.account_guards()
        leaked, why = self.console.account_health_leak(guards.safe_error()[0])
        self.assertFalse(leaked, why)

    def test_the_ref_field_is_pinned_to_the_shape_new_ref_emits(self):
        """`ref` is fixed-width hex, so it does not fall back to the loose token rule — a
        token-shaped value in this field is not a correlation id, whatever else it is."""
        guards = self.console.account_guards()
        clean = {"ok": False, "error": "backend_unavailable", "ref": guards.new_ref()}
        self.assertFalse(self.console.account_health_leak(clean)[0])
        for bad in ("mia_sales_token", "zzzzzzzzzzzz", guards.new_ref()[:6],
                    guards.new_ref() + "ab"):
            leaked, why = self.console.account_health_leak({**clean, "ref": bad})
            self.assertTrue(leaked, f"{bad!r} is not the shape new_ref() emits: {why}")

    def test_the_ok_field_must_be_a_bool_and_not_a_token(self):
        self.assertTrue(self.console.account_health_leak({"ok": "true"})[0])

    def test_a_token_shaped_error_value_is_the_known_residual_gap(self):
        """RECORDED, NOT ACCIDENTAL. `safe_error(code=...)` takes any code and enumerates
        none, so `error` can only be shape-checked and a credential-shaped value walks
        through. Pinned here so the limit is visible in the suite rather than discovered on
        a live box; it closes when `service_guards` gains a code vocabulary, and this test
        is what will fail when it does."""
        leaked, _ = self.console.account_health_leak(
            {"ok": False, "error": "mia_sales_token"})
        self.assertFalse(leaked, "the gap has closed — tighten `error` and rewrite this test")

    def test_a_connection_string_without_the_keyword_dsn_is_still_caught(self):
        """The exact gap in the denylist this replaces. A psycopg DSN reaches a caller as
        `host=… port=… user=…`; carrying `host=` but not `dsn=`, it passed a list looking for
        "traceback", "psycopg", "dsn=" and "password"."""
        body = ("could not connect: host=db.internal port=5432 user=accounts "
                "dbname=accounts sslmode=disable")
        self.assertFalse(
            any(word in body.lower() for word in ("traceback", "psycopg", "dsn=", "password")),
            "the old keyword list would have had to miss this for the test to mean anything")
        leaked, why = self.console.account_health_leak({"ok": False, "error": body})
        self.assertTrue(leaked, why)

    def test_a_field_the_service_never_emits_is_a_leak(self):
        leaked, why = self.console.account_health_leak({"ok": True, "dsn": "postgres://x/y"})
        self.assertTrue(leaked, why)
        self.assertIn("dsn", why)

    def test_a_short_leak_is_caught_too_because_length_is_not_the_rule(self):
        """54 characters, and it is the whole credential failure. A value is permitted for
        being a bare token, not for being small."""
        leaked, why = self.console.account_health_leak(
            {"ok": False, "error": "FATAL: password authentication failed for user hunter2"})
        self.assertTrue(leaked, why)

    def test_the_leak_report_does_not_reprint_the_leak(self):
        """The detail is read in a ticket. Naming the field is the finding; pasting the value
        forward would make the console the second place the secret exists."""
        _, why = self.console.account_health_leak(
            {"ok": False, "error": "FATAL: password authentication failed for user hunter2"})
        self.assertNotIn("hunter2", why)
        _, why = self.console.account_health_leak({"ok": True, "dsn": "postgres://u:pw@db/x"})
        self.assertNotIn("pw@db", why)

    def test_the_leak_check_runs_on_the_failure_path_too(self):
        """Where it matters most: `str(e)` on a psycopg error reaching a route that needs no
        credential is a FAILING health response, and this check used to skip those."""
        self.stub_http({"/health": (500, '{"ok": false, "error": "' + "x" * 200 + '"}')})
        self.console.check_account_service(self.rep)
        self.assertEqual(self.check("account.health")["verdict"], "FAIL")
        self.assertEqual(self.check("account.health_redacted")["verdict"], "FAIL")

    def test_an_unreadable_health_body_blocks_the_leak_check_rather_than_passing_it(self):
        self.stub_http({"/health": (200, "not json at all")})
        self.console.check_account_service(self.rep)
        self.assertEqual(self.check("account.health_redacted")["verdict"], "BLOCKED")

    # ── 3. a non-2xx body is a diagnostic, not something to discard ───────────────────

    def test_http_returns_the_body_of_an_error_response(self):
        """`urllib` hands the error body back on the exception; this used to drop it, leaving
        every caller printing a bare `HTTP 503` at the one moment the body was the answer."""
        error = urllib.error.HTTPError(
            "http://x/ready", 503, "Service Unavailable", {},
            io.BytesIO(b'{"ok": false, "problems": ["migration 003 not applied"]}'))
        self.console.urllib.request.urlopen = self._raise(error)
        self.addCleanup(setattr, self.console.urllib.request, "urlopen",
                        urllib.request.urlopen)
        code, body = self.console.http("http://x/ready")
        self.assertEqual(code, 503)
        self.assertIn("migration 003 not applied", body)

    def test_http_caps_an_error_body_like_any_other(self):
        error = urllib.error.HTTPError("http://x/", 500, "boom", {}, io.BytesIO(b"y" * 9000))
        self.console.urllib.request.urlopen = self._raise(error)
        self.addCleanup(setattr, self.console.urllib.request, "urlopen",
                        urllib.request.urlopen)
        code, body = self.console.http("http://x/", limit=100)
        self.assertEqual((code, len(body)), (500, 100))

    def test_a_transport_failure_still_has_no_body(self):
        self.console.urllib.request.urlopen = self._raise(OSError("connection refused"))
        self.addCleanup(setattr, self.console.urllib.request, "urlopen",
                        urllib.request.urlopen)
        self.assertEqual(self.console.http("http://127.0.0.1:9/"), (0, None))

    # ── the MT container name: an authority, or nothing ───────────────────────────────

    def stub_fleet(self, returncode=0, stdout="", stderr=""):
        """Stand in for the `fleet.sh` subprocess. Patched on the console module so the real
        resolver — and the real compose file — are not what these assertions turn on."""
        class Completed:
            pass
        done = Completed()
        done.returncode, done.stdout, done.stderr = returncode, stdout, stderr
        self.addCleanup(setattr, self.console, "subprocess", self.console.subprocess)
        self.console.subprocess = type(
            "S", (), {"run": staticmethod(lambda *a, **k: done),
                      "TimeoutExpired": self.console.subprocess.TimeoutExpired})

    def test_the_container_name_is_the_resolvers_answer_or_nothing(self):
        """NO FALLBACK LITERAL, for the reason CONTRIBUTING.md gives for `MODEL_PIN`: a
        default is the one value that can silently outrank the authority it stands in for.
        Every failure shape used to return "multi-ironclaw-1"."""
        for returncode, stdout, stderr in ((1, "", "!! MT compose not found: …"),
                                           (0, "", ""),
                                           (0, "   \n", "")):
            self.stub_fleet(returncode, stdout, stderr)
            name, why = self.console._mt_container_name()
            self.assertIsNone(name, f"guessed {name!r} instead of reporting {why!r}")
            self.assertIn("fleet.sh", why)

    def test_a_nonzero_exit_from_the_resolver_is_not_a_name(self):
        """`fleet_mt_container_configured` exits non-zero with its reason on stderr when the
        compose file is missing. That return code was not read at all."""
        self.stub_fleet(1, "", "!! MT compose not found: /nope/docker-compose.yml")
        name, why = self.console._mt_container_name()
        self.assertIsNone(name)
        self.assertIn("MT compose not found", why, "the resolver's own reason was dropped")

    def test_a_resolved_name_is_passed_through_unchanged(self):
        self.stub_fleet(0, "multiclaw\n")
        self.assertEqual(self.console._mt_container_name(), ("multiclaw", ""))

    def test_an_unresolvable_container_blocks_the_egress_check(self):
        """A boundary we could not LOCATE is not a boundary we evaluated. The old fallback
        made this read as `no such container: multi-ironclaw-1` — which sends the operator
        after a container that vanished rather than after the resolver that did not answer —
        and on a box where compose's default naming makes that container real, it evaluates
        the wrong one and can report VERIFIED."""
        self.addCleanup(setattr, self.console, "_mt_container_name",
                        self.console._mt_container_name)
        self.console._mt_container_name = lambda: (None, "deploy/lib/fleet.sh did not answer")
        os.environ.pop("MT_CONTAINER", None)
        state = self.console.egress_state()
        self.assertEqual(state["state"], "BLOCKED", state)
        self.assertEqual(state["why"], ["deploy/lib/fleet.sh did not answer"])
        self.assertNotIn("multi-ironclaw-1", json.dumps(state))

        self.console.check_egress_containment(self.rep, live=True)
        check = self.check("egress.network")
        self.assertEqual(check["verdict"], "BLOCKED", check)
        self.assertEqual(self.rep.exit_code, self.console.EXIT_BLOCKED)

    def test_the_env_override_still_wins_over_the_resolver(self):
        """`MT_CONTAINER` is how an operator points the console at a container on a box whose
        compose this checkout does not have. Removing the fallback must not remove that."""
        self.addCleanup(setattr, self.console, "_mt_container_name",
                        self.console._mt_container_name)
        self.console._mt_container_name = lambda: (None, "should not have been asked")
        self.addCleanup(os.environ.pop, "MT_CONTAINER", None)
        os.environ["MT_CONTAINER"] = "container-that-does-not-exist"
        state = self.console.egress_state()
        self.assertEqual(state.get("container"), "container-that-does-not-exist", state)

    def test_a_failing_readiness_body_reaches_the_operator(self):
        """The end an operator sees: the service's own `problems` list instead of `HTTP 503`."""
        self.stub_http({"/health": (200, '{"ok": true}'),
                        "/ready": (503, json.dumps(
                            {"ok": False, "problems": ["migration 003 not applied"]}))})
        self.console.check_account_service(self.rep)
        check = self.check("account.ready")
        self.assertEqual(check["verdict"], "FAIL")
        self.assertIn("migration 003 not applied", check["detail"])
        self.assertNotEqual(check["detail"], "HTTP 503")

    def test_the_json_payload_is_redacted_and_not_only_the_checks(self):
        """`rep.data` carries whole upstream response bodies to stdout. Scrubbing the details
        and not the payload redacts the half an operator reads and none of the half a script
        keeps."""
        rep = self.console.Report("test")
        rep.bind_secrets([IC_TOKEN_A])
        rep.data["account_db"] = {"problems": [f"upstream said: token={IC_TOKEN_A}"],
                                  "count": 3}
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            rep.emit(as_json=True)
        doc = json.loads(out.getvalue())
        self.assertNotIn(IC_TOKEN_A, out.getvalue())
        self.assertEqual(doc["account_db"]["count"], 3, "scrubbing must not retype the data")

    @staticmethod
    def _raise(exc):
        def fake(*_a, **_kw):
            raise exc
        return fake


if __name__ == "__main__":
    unittest.main(verbosity=2)
