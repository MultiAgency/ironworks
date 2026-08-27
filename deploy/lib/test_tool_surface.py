#!/usr/bin/env python3
"""The one reader of a bearer's tool catalog. Offline, stdlib only, no instance.

Run: python3 deploy/lib/test_tool_surface.py

WHY THIS FILE AND NOT A SELFTEST. These assertions lived in a `__main__` block in
tool_surface.py, where nothing ran them: pytest skips a file not named `test_*.py`, and no gate
invoked it by hand. Its only test importers are multi/verify/test_surface_drift.py and
multi/verify/test_egress_closed.py, both of which need a provisioned instance and are outside
CI — so the console's authority for "what counts as egress" had no offline gate at all.

Naming the file this way is what fixes that, and it is the tree's own argument: deploy/ironworks
(`release verify`) refuses a hand-maintained gate list because it "computes release.promotable
from whatever subset someone remembered to add, and the omission is invisible in the artifact —
it reads as a clean run." Adding an explicit invocation somewhere would have been exactly that.
`deploy/lib/test_*.py` is globbed by both runners, so this needed no edit anywhere else.

The selftest also used bare `assert`, which `-O` strips. These do not.

EVERY CASE HERE IS A FAIL-OPEN. A parser that returns {} for a body it does not understand reads
as "nothing is callable", which is indistinguishable from "everything is locked down" — and one
of the four copies this module replaced had drifted into precisely that shape.
"""
import pathlib
import tempfile
import unittest

import tool_surface as ts


def catalog(*entries):
    return {"entries": [{"key": k, "value": v} for k, v in entries]}


class ParseCatalog(unittest.TestCase):
    def test_tool_entries_are_read_and_the_prefix_is_dropped(self):
        st = ts.parse_catalog(catalog(("tool.builtin.http", {"state": "disabled"}),
                                      ("tool.builtin.echo", {"state": "always_allow"}),
                                      ("skill.something", {"state": "x"})), "selftest")
        self.assertEqual(st, {"builtin.http": "disabled", "builtin.echo": "always_allow"})

    def test_an_entry_whose_value_is_not_a_dict_stays_VISIBLE(self):
        """Two of the four copies skipped these. A tool present but oddly shaped was therefore
        invisible to half the fleet, and invisible reads as absent, which reads as safe."""
        st = ts.parse_catalog(catalog(("tool.builtin.odd", "raw-string-state")), "selftest")
        self.assertEqual(st, {"builtin.odd": "raw-string-state"})

    def test_an_empty_catalog_refuses_to_certify(self):
        with self.assertRaises(SystemExit):
            ts.parse_catalog(catalog(), "selftest")

    def test_a_catalog_with_no_tool_entries_refuses_to_certify(self):
        """Skills only. Nothing was learned about the tool surface, so it cannot read as clean."""
        with self.assertRaises(SystemExit):
            ts.parse_catalog(catalog(("skill.x", {})), "selftest")

    def test_a_malformed_body_raises_rather_than_defaulting_to_empty(self):
        """`doc["entries"]` is subscripted on purpose: an error body must abort the caller.
        Defaulting to [] would turn every error response into a silent pass."""
        for body in ({"error": "unauthorized"}, {}, {"entries": None}):
            with self.assertRaises((KeyError, TypeError)):
                ts.parse_catalog(body, "selftest")


class EgressObservedOff(unittest.TestCase):
    def test_it_returns_the_egress_tools_seen_disabled(self):
        st = {"builtin.http": "disabled", "builtin.echo": "always_allow"}
        self.assertEqual(ts.egress_observed_off(st, "selftest"), ["builtin.http"])

    def test_none_observed_disabled_refuses_to_certify(self):
        """The non-vacuity check. Without it a catalog that simply does not CONTAIN the egress
        tools — a rename, a different build, a truncated response — certifies as clean."""
        with self.assertRaises(SystemExit):
            ts.egress_observed_off({"builtin.echo": "disabled"}, "selftest")

    def test_an_egress_tool_left_enabled_is_not_counted_as_off(self):
        with self.assertRaises(SystemExit):
            ts.egress_observed_off({"builtin.http": "always_allow"}, "selftest")

    def test_every_named_egress_tool_counts_on_its_own(self):
        """EGRESS is a list of alternatives, not a required set: a build that carries only one of
        them must still be certifiable, or a taxonomy change reads as a broken surface."""
        for tool in ts.EGRESS:
            self.assertEqual(ts.egress_observed_off({tool: "disabled"}, "selftest"), [tool])


class ReadDenyList(unittest.TestCase):
    """Exported for the two `deploy/broker/` scripts, which are GITIGNORED but present on operator
    boxes and do import this. It was deleted as dead once, on a header that read as "nothing
    imports them" — `find . -type f -exec grep -l` still names confine-actor.sh and
    eval/probe-confinement.sh. Nothing tested it either way until now."""

    def deny(self, text):
        with tempfile.TemporaryDirectory() as d:
            p = pathlib.Path(d) / "deny.txt"
            p.write_text(text)
            return ts.read_deny_list(p)

    def test_ids_are_read_in_order_with_blanks_and_comments_dropped(self):
        self.assertEqual(
            self.deny("# egress\nbuiltin.http\n\n  builtin.http.save  \n\t# indented comment\n"),
            ["builtin.http", "builtin.http.save"])

    def test_an_empty_or_comment_only_file_reads_as_no_ids(self):
        """It returns [], and that is correct — but a deny-list is not the certifying reader.
        parse_catalog is the one that must never see an empty result as an answer."""
        self.assertEqual(self.deny(""), [])
        self.assertEqual(self.deny("# nothing yet\n\n"), [])

    def test_a_missing_file_raises_rather_than_reading_as_empty(self):
        with self.assertRaises(OSError):
            ts.read_deny_list(pathlib.Path(tempfile.gettempdir()) / "no-such-deny-list-4f2a.txt")


class Module(unittest.TestCase):
    def test_the_selftest_did_not_survive_as_a_second_copy(self):
        """These assertions moved here. If the `__main__` block comes back, the two drift, and
        the one nothing runs is the one that will be wrong."""
        src = (pathlib.Path(__file__).resolve().parent / "tool_surface.py").read_text()
        self.assertNotIn('if __name__ == "__main__":', src)
        self.assertNotIn("selftest: PASS", src)


if __name__ == "__main__":
    unittest.main(verbosity=2)
