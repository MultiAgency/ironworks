#!/usr/bin/env python3
"""Deprovisioning binds destructive scope to authenticated Account Service identity."""
import json
import hashlib
import os
import pathlib
import stat
import subprocess
import tempfile
import unittest


ROOT = pathlib.Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "multi" / "provision" / "deprovision.sh"


class DeprovisionScope(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.home = pathlib.Path(self.tmp.name)
        self.clients = self.home / ".agency" / "clients"
        self.clients.mkdir(parents=True)
        self.identities = self.home / "identities.json"
        self.identities.write_text(json.dumps(
            {"account-token": "authenticated-org", "other-token": "wrong-registry-org"}))
        self.log = self.home / "docker.log"
        self.rm_log = self.home / "rm.log"
        self.bin = self.home / "bin"
        self.bin.mkdir()
        self._executable("curl", r'''#!/usr/bin/env python3
import json, os, pathlib, sys
args = sys.argv[1:]
cfg = pathlib.Path(args[args.index("-K") + 1]).read_text() if "-K" in args else ""
out = args[args.index("-o") + 1] if "-o" in args else None
url = next((a for a in reversed(args) if a.startswith("http")), "")
is_account = "X-Service-Token" in cfg and url.endswith("/list_accounts")
code, body = 200, ""
if is_account:
    if os.environ.get("FAKE_AUTH_FAIL") == "1":
        sys.exit(22)
    identities = json.loads(pathlib.Path(os.environ["FAKE_IDENTITIES"]).read_text())
    live = "account-token" in identities
    if not live and os.environ.get("FAKE_REVOCATION_UNVERIFIED") != "1":
        code = 401
    else:
        body = json.dumps({"org": "authenticated-org", "accounts": []})
elif "/api/webchat/v2/admin/users/" in url and "-X" in args:
    code = 204
elif "/v1/responses/" in url:
    code = 401
if out and out != "/dev/null":
    pathlib.Path(out).write_text(body)
elif not out:
    sys.stdout.write(body)
if "-w" in args:
    sys.stdout.write(str(code))
''')
        self._executable("docker", """#!/usr/bin/env bash
printf '%s\\n' "$*" >> "$FAKE_DOCKER_LOG"
if [ "${1:-}" = exec ]; then
  if [[ " $* " != *' -qtA '* ]] && [ -n "${FAKE_DOCKER_FAIL_ONCE:-}" ] \
     && [ ! -f "$FAKE_DOCKER_FAIL_ONCE" ]; then
    : > "$FAKE_DOCKER_FAIL_ONCE"; cat >/dev/null; exit 9
  fi
  # The inventory is ONE query returning the four table counts as four newline-separated
  # fields (psql -qtA -F '\\n'), so the double answers with four zeros, not one.
  case " $* " in *' -qtA '*) cat >/dev/null; printf '0\\n0\\n0\\n0\\n' ;; *) cat >/dev/null ;; esac
fi
exit 0
""")
        self._executable("rm", """#!/usr/bin/env bash
for arg in "$@"; do
  case "$arg" in
    -*) ;;
    *) printf '%s\\n' "$arg" >> "$FAKE_RM_LOG"
       if [ -n "${FAKE_RM_FAIL_PATH:-}" ] && [ "$arg" = "$FAKE_RM_FAIL_PATH" ]; then
         exit 1
       fi ;;
  esac
done
exec /bin/rm "$@"
""")
        self._executable("systemctl", """#!/usr/bin/env bash
[ "${FAKE_SERVICE_UNAVAILABLE:-0}" != 1 ] || exit 1
printf 'LoadState=%s\nActiveState=%s\nSubState=%s\nMainPID=%s\n' \
  "${FAKE_SERVICE_LOAD:-loaded}" "${FAKE_SERVICE_STATE:-inactive}" \
  "${FAKE_SERVICE_SUBSTATE:-dead}" "${FAKE_SERVICE_PID:-0}"
""")
        self._executable("ps", """#!/usr/bin/env bash
[ "${FAKE_PS_UNAVAILABLE:-0}" != 1 ] || exit 1
printf '%s\n' "${FAKE_PROCESS_STARTED:-Thu Aug 27 12:00:00 2026}"
""")

    def tearDown(self):
        self.tmp.cleanup()

    def _executable(self, name, body):
        path = self.bin / name
        path.write_text(body)
        path.chmod(path.stat().st_mode | stat.S_IXUSR)

    def _registry(self):
        path = self.clients / "acme.env"
        path.write_text(
            "CLIENT_SLUG=acme\nORG_ID=wrong-registry-org\n"
            "ACCOUNT_TOKEN=account-token\nIRONCLAW_TOKEN=member-token\n"
            "TELEGRAM_GROUP_ID=-1007\nIRONCLAW_USER_ID=user-7\n")
        return path

    def _run(self, *args, auth_fail=False, revocation_unverified=False, interrupt_file=None,
             rm_fail_path=None, service_state="inactive", service_pid=0,
             service_unavailable=False, process_started="Thu Aug 27 12:00:00 2026",
             ps_unavailable=False):
        env = dict(os.environ)
        env.update({
            "HOME": str(self.home),
            # HOME and AGENCY_DIR are independent production knobs. Always override both so an
            # operator's ambient custom root cannot receive test receipts or teardown state.
            "AGENCY_DIR": str(self.home / ".agency"),
            # ...and the bridge store, for the identical reason and one the ambient environment
            # really does violate: `multi/seam/test_telegram_bridge.py` sets BRIDGE_STATE
            # process-wide and does not restore it, so in a full-suite run this script inherited
            # a path into another test's deleted tempdir, found no database, and skipped the
            # bridge teardown entirely — passing every assertion by not doing the work. Pinning
            # it here makes these tests independent of collection order.
            "BRIDGE_STATE": str(self.home / ".agency" / "bridge-threads.json"),
            "BRIDGE_STATE_DB": str(self.home / ".agency" / "bridge-threads.db"),
            "CLIENTS_DIR": str(self.clients),
            "IDENTITIES_FILE": str(self.identities),
            "ACCOUNT_DB_CONTAINER": "fake-account-db",
            "FAKE_DOCKER_LOG": str(self.log),
            "FAKE_AUTH_FAIL": "1" if auth_fail else "0",
            "FAKE_REVOCATION_UNVERIFIED": "1" if revocation_unverified else "0",
            "FAKE_IDENTITIES": str(self.identities),
            "FAKE_DOCKER_FAIL_ONCE": str(interrupt_file) if interrupt_file else "",
            "FAKE_RM_LOG": str(self.rm_log),
            "FAKE_RM_FAIL_PATH": str(rm_fail_path) if rm_fail_path else "",
            "FAKE_SERVICE_STATE": service_state,
            "FAKE_SERVICE_SUBSTATE": "running" if service_state == "active" else "dead",
            "FAKE_SERVICE_PID": str(service_pid),
            "FAKE_SERVICE_UNAVAILABLE": "1" if service_unavailable else "0",
            "FAKE_PROCESS_STARTED": process_started,
            "FAKE_PS_UNAVAILABLE": "1" if ps_unavailable else "0",
            "WEBUI_TOKEN": "operator-token",
            "PATH": str(self.bin) + os.pathsep + env["PATH"],
        })
        return subprocess.run(["bash", str(SCRIPT), "acme", *args], cwd=ROOT, env=env,
                              capture_output=True, text=True)

    def _bridge_db(self, openable=True):
        """A bridge store holding this group's conversation, optionally unopenable.

        `openable=False` stamps a future schema version, which `BridgeState` refuses by design —
        the cheapest faithful stand-in for every real way the store can be unreadable at teardown
        (a lock, a corrupt page, a partially-applied migration). The row and its journal stay on
        disk either way, which is the point: they are what "removed" has to mean.
        """
        import sqlite3
        import sys
        sys.path.insert(0, str(ROOT / "multi" / "seam"))
        import bridge_state as bs
        path = self.home / ".agency" / "bridge-threads.db"
        path.parent.mkdir(parents=True, exist_ok=True)
        st = bs.BridgeState(path)
        st.db.execute("INSERT INTO threads(gid, prev) VALUES('-1007','resp_live')")
        st.db.commit()
        st.close()
        if not openable:
            db = sqlite3.connect(str(path))
            db.execute("UPDATE meta SET value=? WHERE key='schema_version'",
                       (str(bs.SCHEMA_VERSION + 5),))
            db.commit(); db.close()
        return path

    def _thread_rows(self, path):
        import sqlite3
        db = sqlite3.connect(str(path))
        n = db.execute("SELECT count(*) FROM threads WHERE gid='-1007'").fetchone()[0]
        db.close()
        return n

    def test_a_bridge_state_deletion_failure_is_degraded_and_keeps_retry_authority(self):
        """`BR_REMOVED=$(… || echo 0)` turned a FAILED delete into the same word as "there was
        nothing there". Nothing set DEGRADED, so teardown went on to remove the registry — the
        only record of TELEGRAM_GROUP_ID — and the run exited 0. The group's conversation pointer
        and its journal were still in the store, and the identifier needed to remove them was
        gone. "Removed 0" and "could not remove" must not print, or exit, identically."""
        registry = self._registry()
        db = self._bridge_db(openable=False)
        result = self._run("--execute", "--confirm", "acme")
        self.assertNotEqual(result.returncode, 0,
                            "a failed bridge-state deletion reported success:\n" + result.stdout)
        self.assertIn("bridge", result.stderr.lower())
        self.assertTrue(registry.exists(),
                        "the registry — the only record of the group id — was removed after a "
                        "bridge-state deletion that did not happen, so the retry has no target")
        self.assertEqual(self._thread_rows(db), 1, "the row vanished; the fixture is wrong")
        self.assertNotIn("bridge entry removed: 1", result.stdout)

    def test_a_bridge_state_deletion_failure_converges_on_rerun(self):
        """The whole reason to retain the registry: fix the store, re-run, converge."""
        import sqlite3
        import sys
        sys.path.insert(0, str(ROOT / "multi" / "seam"))
        import bridge_state as bs
        registry = self._registry()
        db = self._bridge_db(openable=False)
        first = self._run("--execute", "--confirm", "acme")
        self.assertNotEqual(first.returncode, 0)
        self.assertTrue(registry.exists())

        fix = sqlite3.connect(str(db))          # the operator repairs the store
        fix.execute("UPDATE meta SET value=? WHERE key='schema_version'",
                    (str(bs.SCHEMA_VERSION),))
        fix.commit(); fix.close()

        second = self._run("--execute", "--confirm", "acme")
        self.assertEqual(second.returncode, 0,
                         "the rerun did not converge:\n" + second.stdout + second.stderr)
        self.assertEqual(self._thread_rows(db), 0, "the rerun left the group still routable")
        self.assertFalse(registry.exists(), "the converged rerun kept the registry")

    def _set_bridge_pid(self, db, pid):
        """Record a pid in the store's meta, the way a running bridge does."""
        import sqlite3
        c = sqlite3.connect(str(db))
        c.execute("INSERT INTO meta(key,value) VALUES('pid',?) "
                  "ON CONFLICT(key) DO UPDATE SET value=excluded.value", (str(pid),))
        c.commit(); c.close()

    def test_no_running_bridge_means_no_in_memory_route(self):
        """The simplest way the third layer can be absent, and the one a laptop takes."""
        self._registry()
        db = self._bridge_db(openable=True)
        self._set_bridge_pid(db, 999999)          # a pid nothing is using
        result = self._run("--execute", "--confirm", "acme")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("route absence positively established", result.stdout)
        self.assertIn("in-memory route: absent", result.stdout)

    def test_a_bridge_started_before_the_registry_left_still_holds_the_route(self):
        """THE INVARIANT. `telegram_bridge.main()` reads the registry ONCE at startup and holds
        the group dict for the life of the process — no per-group drop, no reload, no signal. So
        a process that was already running still dispatches a group whose registry entry has just
        been deleted, and a deprovision that exits 0 there has not ended routing. This script does
        not restart anything (one process serves every tenant); it refuses to claim success."""
        self._registry()
        db = self._bridge_db(openable=True)
        self._set_bridge_pid(db, os.getpid())     # this test process: started long ago
        result = self._run("--execute", "--confirm", "acme", service_state="active",
                           service_pid=os.getpid())
        self.assertNotEqual(result.returncode, 0,
                            "reported success while the running bridge still held the group:\n"
                            + result.stdout)
        self.assertIn("still holds the group in memory", result.stderr)
        self.assertIn("in-memory route: present", result.stdout)
        self.assertIn("systemctl restart", result.stderr)

    def test_the_rerun_after_a_restart_converges_without_a_registry(self):
        """The receipt is what makes that refusal recoverable: step 6 runs AFTER the registry is
        gone, so the rerun has no registry to read the group id from."""
        import json as _json
        self._registry()
        db = self._bridge_db(openable=True)
        self._set_bridge_pid(db, os.getpid())
        first = self._run("--execute", "--confirm", "acme", service_state="active",
                          service_pid=os.getpid())
        self.assertNotEqual(first.returncode, 0)
        self.assertFalse((self.clients / "acme.env").exists(), "the registry survived step 5")

        receipt = _json.loads(
            (self.home / ".agency" / "deprovision" / "acme.json").read_text())
        self.assertEqual(receipt.get("group_id"), "-1007",
                         f"the receipt did not retain the group id: {sorted(receipt)}")
        self.assertTrue(receipt.get("registry_removed_at"))

        # The replacement's authoritative service PID agrees with bridge metadata and its start
        # epoch is after the retained registry-removal time.
        self._set_bridge_pid(db, os.getpid())
        second = self._run("--execute", "--confirm", "acme", service_state="active",
                           service_pid=os.getpid(),
                           process_started="Fri Aug 28 12:00:00 2026")
        self.assertEqual(second.returncode, 0,
                         "the rerun did not converge:\n" + second.stdout + second.stderr)
        self.assertIn("in-memory route: absent", second.stdout)

    def test_active_service_with_missing_pid_metadata_is_unknown_not_absent(self):
        self._registry(); self._bridge_db(openable=True)
        result = self._run("--execute", "--confirm", "acme", service_state="active",
                           service_pid=os.getpid())
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("PID metadata is missing", result.stderr)
        self.assertIn("in-memory route: unknown", result.stdout)

    def test_active_service_with_malformed_pid_metadata_is_unknown_not_absent(self):
        self._registry(); db = self._bridge_db(openable=True)
        self._set_bridge_pid(db, "not-a-pid")
        result = self._run("--execute", "--confirm", "acme", service_state="active",
                           service_pid=os.getpid())
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("PID metadata is malformed", result.stderr)

    def test_unavailable_authoritative_service_state_is_unknown_not_absent(self):
        self._registry(); self._bridge_db(openable=True)
        result = self._run("--execute", "--confirm", "acme", service_unavailable=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("authoritative service state is unavailable", result.stderr)

    def test_active_service_pid_contradiction_is_unknown_not_absent(self):
        self._registry(); db = self._bridge_db(openable=True)
        self._set_bridge_pid(db, os.getpid())
        result = self._run("--execute", "--confirm", "acme", service_state="active",
                           service_pid=os.getpid() + 1)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("contradicts bridge-state PID", result.stderr)

    def test_unreadable_process_start_time_is_unknown_not_absent(self):
        self._registry(); db = self._bridge_db(openable=True)
        self._set_bridge_pid(db, os.getpid())
        result = self._run("--execute", "--confirm", "acme", service_state="active",
                           service_pid=os.getpid(), ps_unavailable=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("process start time is unavailable", result.stderr)

    def test_a_successful_teardown_really_removes_the_bridge_thread(self):
        """POSITIVE CONTROL. The two tests above would also pass against a script that never
        touched the bridge at all."""
        registry = self._registry()
        db = self._bridge_db(openable=True)
        result = self._run("--execute", "--confirm", "acme")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(self._thread_rows(db), 0,
                         "teardown reported success with the group still in the bridge store")
        self.assertFalse(registry.exists())
        self.assertIn("bridge entry removed: 1", result.stdout)

    def _local_cleanup_order(self):
        if not self.rm_log.exists():
            return []
        return [pathlib.Path(p) for p in self.rm_log.read_text().splitlines()
                if pathlib.Path(p).parent == self.clients or self.clients in pathlib.Path(p).parents]

    def test_stale_registry_org_cannot_redirect_inventory_or_deletion(self):
        registry = self._registry()
        result = self._run("--execute", "--confirm", "acme")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        combined = result.stdout + result.stderr
        self.assertIn("authenticated scope is 'authenticated-org'", combined)
        self.assertNotIn("inventory for client 'acme' (org 'wrong-registry-org')", combined)
        docker_calls = self.log.read_text()
        self.assertIn("org=authenticated-org", docker_calls)
        self.assertNotIn("org=wrong-registry-org", docker_calls)
        remaining = json.loads(self.identities.read_text())
        self.assertNotIn("account-token", remaining,
                         "the authenticated org identity was not removed")
        self.assertEqual(remaining, {"other-token": "wrong-registry-org"},
                         "registry metadata redirected identity removal")
        self.assertFalse(registry.exists(), "the requested tenant registry file survived")
        self.assertIn("VERIFIED REVOKED: old Account token", combined)

    def test_ambient_custom_agency_dir_cannot_receive_fixture_writes(self):
        self._registry()
        with tempfile.TemporaryDirectory() as outside:
            outside = pathlib.Path(outside)
            sentinel = outside / "operator-state"
            sentinel.write_text("untouched")
            previous = os.environ.get("AGENCY_DIR")
            os.environ["AGENCY_DIR"] = str(outside)
            try:
                result = self._run("--execute", "--confirm", "acme")
            finally:
                if previous is None:
                    os.environ.pop("AGENCY_DIR", None)
                else:
                    os.environ["AGENCY_DIR"] = previous

            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertEqual(list(outside.iterdir()), [sentinel],
                             "deprovision fixture wrote into ambient AGENCY_DIR")
            self.assertEqual(sentinel.read_text(), "untouched")
            self.assertTrue((self.home / ".agency/deprovision/acme.json").is_file())

    def test_authentication_failure_aborts_before_any_destructive_work(self):
        registry = self._registry()
        before = self.identities.read_text()
        result = self._run("--execute", "--confirm", "acme", auth_fail=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("cannot authenticate deletion scope", result.stderr)
        self.assertTrue(registry.exists(), "registry was removed before scope authentication")
        self.assertEqual(self.identities.read_text(), before,
                         "identity state changed before scope authentication")
        self.assertFalse(self.log.exists(), "database inventory/destruction ran before authentication")

    def test_dry_run_over_historical_v2_is_filesystem_observational(self):
        import sqlite3
        import sys
        sys.path.insert(0, str(ROOT / "multi" / "seam"))
        from test_thread_compatibility import make_v2

        self._registry()
        path = self.home / ".agency" / "bridge-threads.db"
        make_v2(path, active=True)

        def snapshot():
            root = self.home / ".agency"
            return {str(p.relative_to(root)): (hashlib.sha256(p.read_bytes()).hexdigest(),
                                                stat.S_IMODE(p.stat().st_mode))
                    for p in root.rglob("*") if p.is_file()}

        before = snapshot()
        result = self._run()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("DRY RUN ONLY", result.stdout)
        self.assertEqual(snapshot(), before,
                         "dry-run changed registry, DB/WAL/SHM, receipt or backup files")
        db = sqlite3.connect(str(path))
        self.assertEqual(db.execute(
            "SELECT value FROM meta WHERE key='schema_version'").fetchone()[0], "2")
        db.close()
        self.assertFalse((self.home / ".agency/deprovision/acme.json").exists())

    def test_unverified_account_revocation_is_nonzero_and_retains_registry(self):
        registry = self._registry()
        result = self._run("--execute", "--confirm", "acme", revocation_unverified=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("revocation UNVERIFIED", result.stderr)
        self.assertTrue(registry.exists(), "retry credential was discarded without revocation proof")
        receipt = json.loads((self.home / ".agency/deprovision/acme.json").read_text())
        self.assertEqual(receipt["state"], "authenticated")

    def test_interruption_after_identity_removal_retains_registry_and_rerun_converges(self):
        registry = self._registry()
        marker = self.home / "interrupt.once"
        first = self._run("--execute", "--confirm", "acme", interrupt_file=marker)
        self.assertNotEqual(first.returncode, 0)
        self.assertTrue(registry.exists(), "registry was removed before remote/data teardown finished")
        self.assertNotIn("account-token", json.loads(self.identities.read_text()))
        receipt = json.loads((self.home / ".agency/deprovision/acme.json").read_text())
        self.assertEqual(receipt["state"], "account_revoked")

        second = self._run("--execute", "--confirm", "acme")
        self.assertEqual(second.returncode, 0, second.stdout + second.stderr)
        self.assertFalse(registry.exists())

    def test_guidance_remove_failure_retains_registry_and_rerun_converges(self):
        registry = self._registry()
        guidance = self.clients / "acme.guidance.md"
        guidance.write_text("retry-sensitive guidance")

        first = self._run("--execute", "--confirm", "acme", rm_fail_path=guidance)
        self.assertNotEqual(first.returncode, 0)
        self.assertTrue(guidance.exists(), "the injected guidance failure did not occur")
        self.assertTrue(registry.exists(), "registry retry authority was removed after guidance failed")
        retained = registry.read_text()
        for value in ("account-token", "member-token", "-1007", "user-7"):
            self.assertIn(value, retained, f"retry identifier {value!r} was not retained")
        self.assertNotIn(registry, self._local_cleanup_order(),
                         "registry removal was attempted after guidance cleanup failed")

        self.rm_log.unlink(missing_ok=True)
        second = self._run("--execute", "--confirm", "acme")
        self.assertEqual(second.returncode, 0, second.stdout + second.stderr)
        self.assertFalse(guidance.exists())
        self.assertFalse(registry.exists())
        self.assertEqual(self._local_cleanup_order()[-2:], [guidance, registry])

    def test_registry_remove_failure_is_safely_rerunnable(self):
        registry = self._registry()
        guidance = self.clients / "acme.guidance.md"
        guidance.write_text("cleanup before registry")

        first = self._run("--execute", "--confirm", "acme", rm_fail_path=registry)
        self.assertNotEqual(first.returncode, 0)
        self.assertFalse(guidance.exists(), "guidance was not removed before the registry attempt")
        self.assertTrue(registry.exists(), "the injected registry failure did not retain authority")
        self.assertEqual(self._local_cleanup_order()[-2:], [guidance, registry])

        self.rm_log.unlink(missing_ok=True)
        second = self._run("--execute", "--confirm", "acme")
        self.assertEqual(second.returncode, 0, second.stdout + second.stderr)
        self.assertFalse(registry.exists())

    def test_success_removes_active_registry_after_all_other_registry_artifacts(self):
        registry = self._registry()
        guidance = self.clients / "acme.guidance.md"
        guidance.write_text("cleanup first")
        staged = self.clients / ".staging" / "acme.env"
        staged.parent.mkdir()
        staged.write_text(registry.read_text())

        result = self._run("--execute", "--confirm", "acme")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(self._local_cleanup_order()[-3:], [guidance, staged, registry])
        self.assertFalse(guidance.exists())
        self.assertFalse(staged.exists())
        self.assertFalse(registry.exists())

    def test_fully_absent_rerun_is_idempotent(self):
        self._registry()
        first = self._run("--execute", "--confirm", "acme")
        self.assertEqual(first.returncode, 0, first.stdout + first.stderr)
        second = self._run("--execute", "--confirm", "acme")
        self.assertEqual(second.returncode, 0, second.stdout + second.stderr)
        self.assertIn("already removed", second.stdout + second.stderr)

    def test_missing_registry_without_authenticated_receipt_fails_closed(self):
        before = self.identities.read_text()
        result = self._run("--execute", "--confirm", "acme")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("no authenticating ACCOUNT_TOKEN", result.stderr)
        self.assertEqual(self.identities.read_text(), before)


if __name__ == "__main__":
    unittest.main(verbosity=2)
