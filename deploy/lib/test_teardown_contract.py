#!/usr/bin/env python3
"""The one rule both teardown paths read an HTTP code by. Offline, stdlib only, no network.

WHY THIS EXISTS. Deleting a sealed member happens in two places and they wrote it twice:
`multi/provision/provision.sh`'s compensator, undoing what one failed run created, and
`multi/provision/deprovision.sh`, removing a tenant for good. Both ask the same question — is
the record gone? — and both must answer it the same way, because "gone" is what decides whether
residual authority gets recorded.

They had diverged in a way that only showed up when the instance was unreachable. `curl` without
`-f` exits 0 on 4xx/5xx but NON-ZERO when the request never left. provision.sh guarded that;
deprovision.sh did not, and runs under `set -euo pipefail` — so an unreachable instance aborted
deprovisioning at the DELETE, after the org token had already been deregistered in the previous
step. Half-torn-down tenant, no audit line, non-zero exit that named the wrong thing.

`fleet_member_is_gone` is a pure function over that code, so it is tested here directly rather
than by standing up an instance. `000` being NOT gone is the case worth pinning: it is the only
one where nothing was learned, and reading it as success is precisely how a live member record
would go unrecorded.
"""
import pathlib
import subprocess
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[2]
FLEET = ROOT / "deploy" / "lib" / "fleet.sh"

# A port nothing listens on, so curl fails at the connect phase — the state every `000` in this
# tree is about. `--max-time` keeps a firewalled host from hanging the suite.
REFUSED = "http://127.0.0.1:9/never"


def shell(snippet):
    """Run a snippet with fleet.sh sourced, under the callers' own `set -euo pipefail`, and
    return its stdout stripped. The `set` matters: half of what these helpers exist for is not
    aborting the assignment, which only a pipefail shell can demonstrate."""
    r = subprocess.run(
        ["bash", "-c", f'set -euo pipefail; . "{FLEET}"; {snippet}'],
        capture_output=True, text=True)
    if r.returncode != 0:
        raise AssertionError(f"snippet exited {r.returncode}: {r.stderr.strip()}")
    return r.stdout.strip()


def is_gone(code):
    """Run the shell rule for real. Testing a reimplementation would prove nothing about the
    function the two teardown paths actually call — the same reasoning as test_pins.py running
    fleet.sh's pin reader instead of mirroring it."""
    r = subprocess.run(
        ["bash", "-c", f'set -eu; . "{FLEET}"; fleet_member_is_gone "{code}"'],
        capture_output=True, text=True)
    return r.returncode == 0


class TeardownContract(unittest.TestCase):
    def test_success_and_already_absent_both_count_as_gone(self):
        """404 is the desired end state, not a failure: tearing down an already-deleted tenant
        must be a no-op. The compensator and deprovisioning both re-run against partial state."""
        for code in ("200", "202", "204", "404"):
            self.assertTrue(is_gone(code), f"HTTP {code} should read as gone")

    def test_a_request_that_never_left_is_not_gone(self):
        """THE CASE THAT DIVERGED. `000` is curl's code for "no response" — refused connection,
        DNS failure, timeout. Nothing was learned about the record, so it cannot be success: a
        live sealed member would otherwise be reported as deleted and never enter the residual
        ledger, which is the one place an operator would later look for it."""
        self.assertFalse(is_gone("000"), "a transport failure must never read as deleted")

    def test_an_error_response_is_not_gone(self):
        """A stale or ambient operator token answers 401/403; the record is untouched."""
        for code in ("400", "401", "403", "409", "500", "502"):
            self.assertFalse(is_gone(code), f"HTTP {code} should not read as gone")

    def test_a_refused_connection_yields_exactly_000(self):
        """THE DEFECT `fleet_http_code` EXISTS FOR, measured against a real refused socket.

        `|| echo 000` appends to the `000` curl already wrote, producing `000000` — a value that
        equals nothing, so every `[ "$code" = "000" ]` guard written against it is unreachable and
        the caller falls through to its else branch. That branch is not neutral: in deprovision.sh
        it records residual authority nobody measured. The assertion is on the exact string for
        that reason — `000000` is truthy, non-empty, and looks like a code."""
        got = shell(f'fleet_http_code curl -s -o /dev/null -w "%{{http_code}}" '
                    f'--max-time 5 "{REFUSED}"')
        self.assertEqual(got, "000",
                         "a refused connection must read as exactly 000, not an appended variant")

    def test_the_wrong_idiom_is_gone_from_every_script(self):
        """The class, not the instance. Eight live sites carried `|| echo 000` while two files in
        the same tree documented at length why it is wrong — the rule was written down and the
        callers never converted. Source-level is the only way to catch the ninth copy, because a
        behavioural test only sees the paths a fixture happens to drive."""
        offenders = []
        for path in sorted(ROOT.glob("**/*.sh")):
            if ".git" in path.parts:
                continue
            for n, line in enumerate(path.read_text().splitlines(), 1):
                if "|| echo 000" in line and not line.lstrip().startswith("#"):
                    offenders.append(f"{path.relative_to(ROOT)}:{n}")
        self.assertEqual(offenders, [],
                         "use `fleet_http_code`; `|| echo 000` yields the literal 000000")

    def test_both_teardown_paths_use_the_shared_helpers(self):
        """Source-level, because the alternative is provisioning a tenant to find out. If either
        path goes back to its own `curl ... -X DELETE` and its own case arms, they can drift
        again exactly as they did — and the drift is invisible until an instance is unreachable
        mid-teardown."""
        for name in ("provision.sh", "deprovision.sh"):
            src = (ROOT / "multi" / "provision" / name).read_text()
            self.assertIn("fleet_delete_member", src,
                          f"{name} no longer deletes through the shared helper")
            self.assertIn("fleet_member_is_gone", src,
                          f"{name} no longer reads the code through the shared rule")
            self.assertNotIn("admin/users/$IRONCLAW_USER_ID", src,
                             f"{name} still builds the DELETE URL itself")


if __name__ == "__main__":
    unittest.main(verbosity=2)
