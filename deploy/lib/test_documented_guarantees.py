#!/usr/bin/env python3
"""The documented delivery guarantee must equal the implemented one.

WHY THIS EXISTS. `deploy/lib/test_doc_refs.py` gates every repo-relative PATH the docs name, so a
citation can never point at a file that is not there. Nothing gated what the docs SAY. Two claims
in the two most authoritative documents had drifted, and both were found by reading rather than by
any check:

  * `SECURITY.md` stated "Ordering | Strict within a group; serial across groups" and "cross-group
    work is serial", while `bridge_core.Bridge` has been running a bounded worker pool across
    groups. `docs/BRIDGE_DELIVERY.md` had it right — the two files each carried a copy of the same
    guarantee table, and the copy is the one that went stale.
  * `docs/BRIDGE_DELIVERY.md` described v1 -> v2 as the current migration and told an operator that
    a code rollback "requires restoring that v1 backup", while `bridge_state.SCHEMA_VERSION` was
    3 — and a database *born* at v2 has no v1 backup to restore. A recovery instruction that does
    not apply to a live class of database is worse than none.

WHAT IT CHECKS, AND WHY ONLY THIS MUCH. Four claims about the bridge, chosen because each is
mechanically derivable from code and each is one an operator acts on during an incident. It
deliberately does not try to gate prose in general: `test_doc_refs.py`'s header records that a
gate gets switched off once it is noisy enough, and a checker that tried to verify every sentence
would be that gate.

The concurrency bound is taken by CONSTRUCTING a `Bridge` and reading what it settled on, not by
grepping `bridge_core.py` for literals. Asserting source text against doc text pins two spellings
to each other and lets both drift from behaviour together — which is the same defect class as the
prose this file exists to catch.

Run:  python3 deploy/lib/test_documented_guarantees.py
"""
import os
import pathlib
import re
import subprocess
import sys
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "multi" / "seam"))

import bridge_core  # noqa: E402  path set above
import bridge_state  # noqa: E402

DELIVERY = ROOT / "docs" / "BRIDGE_DELIVERY.md"
# The delivery guarantee table's header row. This exact string is what was duplicated.
TABLE_HEADER = "| Property | Guarantee |"


def _tracked_markdown():
    """Tracked `*.md`, resolved against the INDEX for the same reason `test_doc_refs.py` does:
    the index is what a clone gets, so it is what the docs may promise a reader."""
    out = subprocess.run(["git", "ls-files", "-z", "--", "*.md"],
                         cwd=ROOT, capture_output=True, text=True, check=True).stdout
    return [ROOT / p for p in out.split("\0") if p]


class DocumentedGuarantees(unittest.TestCase):
    def setUp(self):
        self._saved = os.environ.pop("BRIDGE_MAX_WORKERS", None)

    def tearDown(self):
        if self._saved is not None:
            os.environ["BRIDGE_MAX_WORKERS"] = self._saved

    def _bridge(self, max_workers=None):
        """A Bridge built only far enough to have settled its worker bound.

        Every collaborator is assigned and none is touched in `__init__`, so `None` is honest
        here — this drives the real clamp rather than a copy of it.
        """
        return bridge_core.Bridge(groups={}, threads={}, telegram=None, turns=None, state=None,
                                  clock=None, log=None, budget_seconds=1, max_workers=max_workers)

    def test_the_documented_state_names_are_the_implemented_ones(self):
        """Every state the doc's transition diagram names must exist, and none may be missing.

        Both directions matter: a state removed from the code leaves the doc promising a
        transition that cannot happen, and a state added without documenting it leaves an
        operator meeting `RECOVERY_BLOCKED` for the first time during the incident it names.
        """
        block = re.search(r"## State transitions\s*```text\n(.*?)```", DELIVERY.read_text(), re.S)
        self.assertIsNotNone(block, "BRIDGE_DELIVERY.md has no '## State transitions' text block")
        documented = set(re.findall(r"\b[A-Z][A-Z_]{3,}\b", block.group(1)))
        implemented = set(bridge_state.ALL_STATES)
        self.assertEqual(implemented, documented,
                         f"undocumented: {sorted(implemented - documented)}; "
                         f"documented but not implemented: {sorted(documented - implemented)}")

    def test_the_documented_schema_version_is_the_implemented_one(self):
        """The rollback section tells an operator which backup to restore. It named v2 while the
        code was at v3, and the advice it gave does not apply to a database born at v2."""
        m = re.search(r"current schema is \*\*v(\d+)\*\*", DELIVERY.read_text())
        self.assertIsNotNone(m, "BRIDGE_DELIVERY.md no longer states 'The current schema is **vN**'")
        self.assertEqual(bridge_state.SCHEMA_VERSION, int(m.group(1)),
                         "the documented schema version is not the one the bridge implements")

    def test_the_documented_concurrency_bounds_are_the_implemented_ones(self):
        """`default N, maximum M` in the ordering row, against what a Bridge actually settles on."""
        m = re.search(r"default (\d+), maximum (\d+)", DELIVERY.read_text())
        self.assertIsNotNone(m, "BRIDGE_DELIVERY.md no longer states 'default N, maximum M'")
        documented_default, documented_max = int(m.group(1)), int(m.group(2))
        self.assertEqual(documented_default, self._bridge().max_workers,
                         "the documented default worker count is not the one the bridge uses")
        # Ask for far more than the ceiling and see where it lands, rather than trusting a literal.
        self.assertEqual(documented_max, self._bridge(documented_max * 100).max_workers,
                         "the documented maximum is not the ceiling the bridge clamps to")
        self.assertEqual(1, self._bridge(0).max_workers, "the floor is no longer 1")

    def test_the_delivery_guarantee_table_has_exactly_one_owner(self):
        """The drift was not a typo — it was a second copy. One document owns the table and the
        others cite it, which is the rule `CONTRIBUTING.md` states for the component map and the
        rule this table was the exception to."""
        owners = [p for p in _tracked_markdown() if TABLE_HEADER in p.read_text()]
        rel = sorted(str(p.relative_to(ROOT)) for p in owners)
        self.assertEqual(["docs/BRIDGE_DELIVERY.md"], rel,
                         "the delivery guarantee table must live in exactly one document; "
                         f"found in {rel or 'nothing'}")


if __name__ == "__main__":
    unittest.main(verbosity=2)
