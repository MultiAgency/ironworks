#!/usr/bin/env python3
"""Provisioning journal + residual-authority ledger. Offline, stdlib only.

Run: python3 deploy/lib/test_lifecycle.py

The journal and the ledger both exist to make a partial or finished lifecycle step a FACT on
disk rather than something in an operator's head. Two properties matter more than the rest and
have a test each: they never hold a credential, and they are never briefly world-readable.
"""
import contextlib
import io
import json
import os
import stat
import tempfile
import time
import unittest

import lifecycle as lc


class Base(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self._old = os.environ.get("AGENCY_DIR")
        self._old_ledger = os.environ.get("RESIDUAL_LEDGER")
        os.environ["AGENCY_DIR"] = self._tmp.name
        os.environ.pop("RESIDUAL_LEDGER", None)

    def tearDown(self):
        self._tmp.cleanup()
        for k, v in (("AGENCY_DIR", self._old), ("RESIDUAL_LEDGER", self._old_ledger)):
            if v is None:
                os.environ.pop(k, None)
            else:
                os.environ[k] = v


class Journal(Base):
    def test_stages_advance_and_are_readable(self):
        self.assertEqual(lc.journal_stage("acme"), "")
        lc.journal_set("acme", "preflight_passed")
        lc.journal_set("acme", "org_registered", {"org_id": "acme"})
        self.assertEqual(lc.journal_stage("acme"), "org_registered")
        self.assertEqual(lc.journal_get("acme")["org_id"], "acme")

    def test_history_records_every_transition(self):
        """A journal that only holds the CURRENT stage cannot tell a resumed run from a first
        one, which is the difference between 'this org exists' and 'this org exists twice'."""
        for s in ("preflight_passed", "org_registered", "member_minted"):
            lc.journal_set("acme", s)
        hist = [h["stage"] for h in lc.journal_get("acme")["history"]]
        self.assertEqual(hist, ["preflight_passed", "org_registered", "member_minted"])

    def test_reached_is_ordered_not_equality(self):
        lc.journal_set("acme", "member_confined")
        self.assertTrue(lc.journal_reached("acme", "org_registered"))
        self.assertTrue(lc.journal_reached("acme", "member_confined"))
        self.assertFalse(lc.journal_reached("acme", "smoke_passed"))

    def test_an_unknown_stage_is_refused(self):
        with self.assertRaises(ValueError):
            lc.journal_set("acme", "almost_done")

    def test_credential_shaped_fields_are_refused(self):
        """The journal outlives a crash. A token in it would be a second copy of a client
        credential in exactly the file most likely to be left behind."""
        for bad in ("ironclaw_token", "ACCOUNT_TOKEN", "api_key", "signing_secret",
                    "bearer", "db_password", "credential_id"):
            with self.assertRaises(ValueError, msg=bad):
                lc.journal_set("acme", "org_registered", {bad: "x"})

    def test_long_opaque_values_are_refused(self):
        with self.assertRaises(ValueError):
            lc.journal_set("acme", "org_registered", {"note": "x" * 300})

    def test_the_file_is_0600(self):
        lc.journal_set("acme", "preflight_passed")
        mode = stat.S_IMODE(lc.journal_path("acme").stat().st_mode)
        self.assertEqual(mode, 0o600, oct(mode))

    def test_clear_is_idempotent(self):
        lc.journal_set("acme", "preflight_passed")
        self.assertTrue(lc.journal_clear("acme"))
        self.assertFalse(lc.journal_clear("acme"))
        self.assertEqual(lc.journal_stage("acme"), "")

    def test_a_corrupt_journal_does_not_crash_a_reader(self):
        """The journal is written during failures. A truncated one must not stop the run that
        is trying to clean up after the failure that truncated it."""
        p = lc.journal_path("acme")
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text("{not json")
        self.assertEqual(lc.journal_stage("acme"), "")
        lc.journal_set("acme", "preflight_passed")     # and it recovers on the next write
        self.assertEqual(lc.journal_stage("acme"), "preflight_passed")


class Ledger(Base):
    def test_an_entry_records_expiry_and_no_token(self):
        e = lc.residual_add("acme", {"uid": "user-123", "lifetime_days": "365"})
        self.assertEqual(e["session_lifetime_days"], 365)
        self.assertIn("expires_at", e)
        blob = json.dumps(e)
        self.assertNotIn("token", blob.lower())

    def test_outstanding_and_expired_are_split_on_the_clock(self):
        lc.residual_add("live", {"uid": "u1", "lifetime_days": "365"})
        lc.residual_add("gone", {"uid": "u2", "lifetime_days": "0"})
        out, done = lc.residual_list()
        self.assertEqual(sorted(out), ["live"])
        self.assertEqual(sorted(done), ["gone"])

    def test_drop_is_idempotent(self):
        lc.residual_add("acme", {"uid": "u", "lifetime_days": "1"})
        self.assertTrue(lc.residual_drop("acme"))
        self.assertFalse(lc.residual_drop("acme"))

    def test_the_ledger_is_0600(self):
        lc.residual_add("acme", {"uid": "u", "lifetime_days": "1"})
        mode = stat.S_IMODE(lc.ledger_path().stat().st_mode)
        self.assertEqual(mode, 0o600, oct(mode))

    def test_credential_shaped_fields_are_refused_here_too(self):
        with self.assertRaises(ValueError):
            lc.residual_add("acme", {"member_token": "abc", "lifetime_days": "1"})

    def test_re_deprovisioning_replaces_rather_than_duplicates(self):
        """Deprovisioning must converge: running it twice leaves ONE entry, not two."""
        lc.residual_add("acme", {"uid": "u", "lifetime_days": "365"})
        lc.residual_add("acme", {"uid": "u", "lifetime_days": "365"})
        out, _ = lc.residual_list()
        self.assertEqual(list(out), ["acme"])

    def test_a_good_minted_at_dates_the_expiry_from_the_token_not_the_clock(self):
        """The positive control for the two refusals below: without it they would still pass if
        `minted_at` were ignored entirely, which is the defect they exist to catch."""
        e = lc.residual_add("acme", {"uid": "u", "lifetime_days": "365",
                                     "minted_at": "2026-01-02T03:04:05+00:00"})
        self.assertEqual(e["expires_at"], "2027-01-02T03:04:05+00:00")

    def test_an_unreadable_minted_at_stops_the_record(self):
        """THE FIRST RECORD IS THE ONE THAT CAN GO WRONG. `except ValueError: pass` left the base
        at "now", so a typo'd timestamp produced an expiry computed from the clock — recorded as
        fact, in the one field the ledger exists to state, with nothing printed. The re-record
        guard cannot help: it preserves whatever the first call wrote."""
        for bad in ("not-a-date", "2026-13-01T00:00:00+00:00", "2026-01-02T99:00:00+00:00"):
            with self.assertRaises(ValueError, msg=bad):
                lc.residual_add("acme", {"uid": "u", "lifetime_days": "365", "minted_at": bad})
        self.assertEqual(lc.residual_list()[0], {}, "a refused record still wrote an entry")

    def test_a_naive_minted_at_is_refused_rather_than_assumed_utc(self):
        """`expires_at_epoch` comes from `.timestamp()`, which reads a naive value in LOCAL time
        — so the same input would record a different epoch on a laptop than on the host, beside
        an `expires_at` with no offset where every sibling entry carries `+00:00`."""
        with self.assertRaises(ValueError):
            lc.residual_add("acme", {"uid": "u", "lifetime_days": "365",
                                     "minted_at": "2026-01-02T03:04:05"})

    def test_re_recording_does_not_push_the_recorded_expiry_forward(self):
        """AN AUDIT RECORD IS NOT A MOVING TARGET. The window belongs to a token that was minted
        once; re-recording the entry describes the same token, so recomputing `expires_at` from
        the clock extends a deadline that did not move in reality.

        Only `classification` / `waiver_reason` / `classified_at` were carried over, so two
        calls a second apart moved `expires_at_epoch` by a second and re-stamped `deleted_at`.
        `deprovision.sh:311-320` states this invariant and enforces it ONLY with its own
        ALREADY_ABSENT flag — so a partial-failure re-run that still found the member present,
        or any second caller of this module, pushed the expiry out again.

        This test asserts the field VALUES, because the sibling above asserts only that one
        entry survives and would pass on every version of this defect."""
        first = lc.residual_add("acme", {"uid": "u", "lifetime_days": "365"})
        time.sleep(1.1)
        second = lc.residual_add("acme", {"uid": "u", "lifetime_days": "365"})
        for field in ("expires_at", "expires_at_epoch", "deleted_at", "session_lifetime_days"):
            self.assertEqual(second[field], first[field],
                             f"{field} moved on a re-record: {first[field]} -> {second[field]}")


class Classification(Base):
    """The distinction is only allowed to exist if it is explicit and tested."""

    def test_a_new_entry_defaults_to_ACTIVE_RISK(self):
        e = lc.residual_add("acme", {"uid": "u", "lifetime_days": "365"})
        self.assertEqual(e["classification"], lc.ACTIVE_RISK)
        blocking, waived = lc.residual_split()
        self.assertEqual(sorted(blocking), ["acme"])
        self.assertEqual(waived, {})

    def test_classifying_requires_a_reason(self):
        lc.residual_add("acme", {"uid": "u", "lifetime_days": "365"})
        for bad in ("", "   ", None):
            with self.assertRaises(ValueError):
                lc.residual_classify("acme", lc.TEST_RESIDUAL, bad)

    def test_REVOKED_cannot_be_asserted_by_hand(self):
        """Only a probe that MEASURED a rejection may say revoked. Allowing a human to declare
        it would turn the deprovisioning gate into an honour system — which is the one thing
        the whole residual-authority design exists to prevent."""
        lc.residual_add("acme", {"uid": "u", "lifetime_days": "365"})
        with self.assertRaises(ValueError) as cm:
            lc.residual_classify("acme", lc.REVOKED, "I am sure it is fine")
        self.assertIn("measured", str(cm.exception))
        with self.assertRaises(ValueError):
            lc.residual_classify("acme", lc.EXPIRED, "it looks old")

    def test_a_waived_entry_stays_visible_and_keeps_its_real_expiry(self):
        """A waiver changes what the release gate does. It must never change what the ledger
        says about the token."""
        lc.residual_add("acme", {"uid": "u", "lifetime_days": "365"})
        e = lc.residual_classify("acme", lc.TEST_RESIDUAL, "laptop self-test, never a client")
        self.assertEqual(e["classification"], lc.TEST_RESIDUAL)
        self.assertIn("expires_at", e)
        self.assertEqual(e["waiver_reason"], "laptop self-test, never a client")
        self.assertTrue(e["classified_at"])
        outstanding, _ = lc.residual_list()
        self.assertIn("acme", outstanding, "a waived session vanished from the ledger")
        blocking, waived = lc.residual_split(outstanding)
        self.assertEqual(blocking, {})
        self.assertEqual(sorted(waived), ["acme"])

    def test_re_deprovisioning_does_not_silently_re_arm_a_waiver(self):
        lc.residual_add("acme", {"uid": "u", "lifetime_days": "365"})
        lc.residual_classify("acme", lc.TEST_RESIDUAL, "synthetic")
        again = lc.residual_add("acme", {"uid": "u", "lifetime_days": "365"})
        self.assertEqual(again["classification"], lc.TEST_RESIDUAL)
        self.assertEqual(again["waiver_reason"], "synthetic")

    def test_classifying_an_unknown_slug_is_refused(self):
        with self.assertRaises(ValueError):
            lc.residual_classify("never-existed", lc.TEST_RESIDUAL, "because")

    def test_list_exit_code_reflects_BLOCKING_only(self):
        lc.residual_add("acme", {"uid": "u", "lifetime_days": "365"})
        self.assertEqual(lc.main(["residual", "list"]), 2)
        lc.residual_classify("acme", lc.TEST_RESIDUAL, "synthetic laptop self-test")
        self.assertEqual(lc.main(["residual", "list"]), 0,
                         "a waived entry still blocked the exit code")


class TeardownReceipt(Base):
    def test_scope_and_phase_survive_without_a_credential(self):
        lc.teardown_set("acme", "authenticated",
                        {"org_id": "org-from-service", "account_base": "http://accounts"})
        lc.teardown_set("acme", "account_revoked")
        doc = lc.teardown_get("acme")
        self.assertEqual(doc["org_id"], "org-from-service")
        self.assertEqual(doc["state"], "account_revoked")
        self.assertNotIn("ACCOUNT_TOKEN", lc.teardown_path("acme").read_text())

    def test_receipt_is_private(self):
        lc.teardown_set("acme", "complete", {"org_id": "org-from-service"})
        mode = stat.S_IMODE(lc.teardown_path("acme").stat().st_mode)
        self.assertEqual(mode, 0o600, oct(mode))


class CLI(Base):
    def test_residual_list_exits_2_while_authority_is_outstanding(self):
        """The exit code is what makes an unattended check meaningful — a ledger nobody reads
        is the same as no ledger."""
        lc.residual_add("acme", {"uid": "u", "lifetime_days": "365"})
        self.assertEqual(lc.main(["residual", "list"]), 2)
        lc.residual_drop("acme")
        self.assertEqual(lc.main(["residual", "list"]), 0)

    def test_classify_via_the_cli_records_the_reason(self):
        lc.residual_add("acme", {"uid": "u", "lifetime_days": "365"})
        self.assertEqual(
            lc.main(["residual", "classify", "acme", "TEST_RESIDUAL", "synthetic", "selftest"]), 0)
        e = lc.read_json(lc.ledger_path(), {})["acme"]
        self.assertEqual(e["classification"], "TEST_RESIDUAL")
        self.assertIn("synthetic", e["waiver_reason"])

    def test_cli_refuses_REVOKED_with_exit_64(self):
        lc.residual_add("acme", {"uid": "u", "lifetime_days": "365"})
        self.assertEqual(lc.main(["residual", "classify", "acme", "REVOKED", "trust me"]), 64)

    def test_journal_reached_exit_code(self):
        lc.journal_set("acme", "member_minted")
        self.assertEqual(lc.main(["journal", "reached", "acme", "org_registered"]), 0)
        self.assertEqual(lc.main(["journal", "reached", "acme", "activated"]), 1)

    def test_a_credential_field_from_the_shell_is_refused_with_64(self):
        self.assertEqual(lc.main(["journal", "set", "acme", "org_registered", "token=abc"]), 64)

    def test_exit_1_means_FALSE_and_only_false(self):
        """THE CONTRACT THE SHELL CALLERS READ. `deprovision.sh` runs `residual has <slug>` and
        branches on the code; `provision.sh` runs `journal reached <slug> <stage>` three times to
        decide whether to SKIP creating authority it already created.

        Exit 1 used to mean three different things: this boolean, every usage error ("unknown
        group", "expected key=value"), and six uncaught `IndexError` tracebacks from a missing
        positional. So `if lifecycle.py residual has "$SLUG"; then` could not distinguish "no
        residual authority" from "you typo'd the subcommand" — and the typo took the FALSE branch
        silently, which for `journal reached org_registered` means minting a second live org
        token.

        Asserted as a partition, not case by case: every usage shape must be 64, and the only
        things allowed to be 1 are the two queries."""
        lc.journal_set("acme", "member_minted")
        lc.residual_add("acme", {"uid": "u", "lifetime_days": "365"})

        # The two genuine booleans — 0 for true, 1 for false, and no output either way.
        self.assertEqual(lc.main(["residual", "has", "acme"]), 0)
        self.assertEqual(lc.main(["residual", "has", "nosuch"]), 1)
        self.assertEqual(lc.main(["journal", "reached", "acme", "org_registered"]), 0)
        self.assertEqual(lc.main(["journal", "reached", "acme", "activated"]), 1)

        # Everything a caller can get wrong is 64. A traceback here is also a failure: these all
        # used to reach `rest[0]` on an empty list.
        usage = [
            ["bogus"], ["journal"], ["journal", "bogus"], ["teardown", "bogus"],
            ["residual", "bogus"], ["journal", "get"], ["journal", "stage"],
            ["journal", "clear"], ["journal", "set"], ["journal", "set", "acme"],
            ["journal", "reached", "acme"], ["journal", "reached", "acme", "not_a_stage"],
            ["teardown", "get"], ["teardown", "set", "acme"],
            ["residual", "has"], ["residual", "drop"], ["residual", "add"],
            ["residual", "classify", "acme"],
            ["journal", "set", "acme", "member_minted", "novalue"],
            ["journal", "stage", "acme", "extra"], ["residual", "has", "acme", "extra"],
        ]
        for argv in usage:
            with self.subTest(argv=argv):
                buf = io.StringIO()
                with contextlib.redirect_stderr(buf), contextlib.redirect_stdout(io.StringIO()):
                    try:
                        code = lc.main(argv)
                    except SystemExit as e:            # argparse's own exit path
                        code = e.code
                self.assertEqual(code, 64, f"{argv} exited {code}, not 64: {buf.getvalue()[:120]}")
                self.assertNotIn("Traceback", buf.getvalue(), f"{argv} produced a traceback")

    def test_residual_list_keeps_exit_2_for_outstanding_authority(self):
        """2 is the release gate's signal and must not collide with argparse's own error exit —
        which is why `_Usage.error` overrides it to 64."""
        with contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(lc.main(["residual", "list"]), 0)
            lc.residual_add("acme", {"uid": "u", "lifetime_days": "365"})
            self.assertEqual(lc.main(["residual", "list"]), 2)
            lc.residual_classify("acme", "TEST_RESIDUAL", "synthetic")
            self.assertEqual(lc.main(["residual", "list"]), 0, "a waived entry still blocked")


if __name__ == "__main__":
    unittest.main(verbosity=2)
