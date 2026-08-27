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
