#!/usr/bin/env python3
"""Which service this suite will grade, and which it refuses — offline.

WHY THIS IS TESTABLE AT ALL. `run_eval.py` needs a live instance and the eval org's credentials,
so nothing in it was reachable from a gate. But the DECISION of what to grade is a pure function
of the service definitions, and it is the half that was wrong: the runner composed the default
service unconditionally, so every definition's `evaluation` field was a claim about coverage that
no code could act on. `resolve_service` is that decision, split out so it can be tested without a
model, a token or a network.

WHAT IT PROTECTS. Two refusals whose absence would not look like a failure:

  * grading a service that declares `"evaluation": null` would manufacture the exact coverage the
    null exists to deny, and would do it while printing a plausible score;
  * grading a service that declares a DIFFERENT suite would run account-qualification cases —
    whose evidence-discipline grader greps a four-tier vocabulary only `ANALYST.md` defines —
    against a service with another objective, and report the result as evidence.

Both produce a number either way. That is what makes them worth a test rather than a comment.

Run:  python3 multi/eval/test_service_selection.py
"""
import pathlib
import sys
import unittest

HERE = pathlib.Path(__file__).resolve().parent
ROOT = HERE.parents[1]
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(ROOT / "multi" / "seam"))

import services as svc  # noqa: E402  path set above
import run_eval  # noqa: E402


class ServiceSelection(unittest.TestCase):
    def setUp(self):
        svc._cache.clear()

    def test_the_default_is_the_service_this_suite_claims_to_cover(self):
        """The positive control. Every refusal below is an absence check, and an absence check
        that never sees a passing case cannot distinguish 'correctly refused' from 'refuses
        everything'."""
        d = run_eval.resolve_service()
        self.assertEqual(svc.DEFAULT_SERVICE, d["service"])
        self.assertEqual("multi/eval", d["evaluation"],
                         "the default service no longer declares this suite — the runner would "
                         "now refuse the very thing it exists to grade")

    def test_naming_that_service_explicitly_is_the_same_answer(self):
        self.assertEqual(run_eval.resolve_service(svc.DEFAULT_SERVICE)["service"],
                         run_eval.resolve_service()["service"])

    def test_a_service_declaring_no_evaluation_is_refused_not_graded(self):
        """`relationship-intelligence` declares null on purpose. The runner must say so rather
        than grade it with cases written for a different objective."""
        internal = [n for n in svc.available()
                    if svc.load_service(n)["evaluation"] is None]
        self.assertTrue(internal, "no service declares a null evaluation — this test now proves "
                                  "nothing; check whether that null was filled in")
        with self.assertRaises(SystemExit) as cm:
            run_eval.resolve_service(internal[0])
        message = str(cm.exception)
        self.assertIn("null", message)
        # The refusal must say what the service IS for, or the operator's next move is to guess.
        self.assertIn("responsibility is:", message)

    def test_a_service_declaring_a_different_suite_is_refused(self):
        """A suite grading a service that does not claim it is not evidence."""
        real = svc.load_service(svc.DEFAULT_SERVICE)
        elsewhere = dict(real, evaluation="multi/verify")
        original = svc.load_service
        svc.load_service = lambda name, base=None: elsewhere
        try:
            with self.assertRaises(SystemExit) as cm:
                run_eval.resolve_service(svc.DEFAULT_SERVICE)
            self.assertIn("multi/verify", str(cm.exception))
        finally:
            svc.load_service = original

    def test_an_unknown_service_is_refused_with_the_known_ones_named(self):
        with self.assertRaises(SystemExit) as cm:
            run_eval.resolve_service("no-such-service")
        message = str(cm.exception)
        # Assert the operator is told what IS available, not that a particular prefix is used —
        # `load_service` owns that wording and appending a second copy here would be a second
        # list to keep in step.
        self.assertIn(svc.DEFAULT_SERVICE, message,
                      "the refusal does not name any service the operator could run instead")
        self.assertEqual(1, message.count(svc.DEFAULT_SERVICE),
                         "the known-services list is printed twice")

    def test_the_suite_path_comparison_is_resolved_not_textual(self):
        """`SUITE_DIR` and the declared path must be compared as filesystem locations. A string
        compare would fail on `multi/eval/` vs `multi/eval` and pass on a symlinked lookalike."""
        self.assertEqual(HERE, run_eval.SUITE_DIR)
        self.assertEqual((ROOT / "multi/eval").resolve(), run_eval.SUITE_DIR)


if __name__ == "__main__":
    unittest.main(verbosity=2)
