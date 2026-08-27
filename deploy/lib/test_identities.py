#!/usr/bin/env python3
"""Authoritative identity state — the five cases provisioning must get right.

The defect these cover: provisioning decided whether to register an Account-Service credential
from LIFECYCLE JOURNAL PROVENANCE ("did this tool register one?") rather than from identity
state ("does one exist?"). An org created by any other supported path — `seed-real.sh` creates
one — has no journal, so the tool minted a second live token, overwrote the credential file and
left the first authenticating. Offline, no flask/psycopg, no network.
"""
import json
import pathlib
import subprocess
import sys
import tempfile
import unittest

HERE = pathlib.Path(__file__).resolve().parent
import identities as ident  # noqa: E402

TOK_A = "tok-aaaa"   # gitleaks:allow — fixture literals, not credentials
TOK_B = "tok-bbbb"   # gitleaks:allow
TOK_C = "tok-cccc"   # gitleaks:allow


class IdentityState(unittest.TestCase):
    def _file(self, doc):
        d = pathlib.Path(self._tmp.name) / "identities.json"
        d.write_text(json.dumps(doc))
        return d

    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)

    def _cli(self, action, org, path):
        return subprocess.run([sys.executable, str(HERE / "identities.py"), action, org],
                              capture_output=True, text=True,
                              env={"ACCOUNT_IDENTITIES_FILE": str(path), "PATH": "/usr/bin:/bin"})

    # ── case 1: org absent -> register exactly one ────────────────────────────────────
    def test_absent_org_reports_zero_and_refuses_to_resolve(self):
        p = self._file({TOK_A: "other"})
        self.assertEqual(ident.tokens_for_org(ident.load(p), "multiagency"), [])
        self.assertEqual(self._cli("count", "multiagency", p).stdout.strip(), "0")
        r = self._cli("resolve", "multiagency", p)
        self.assertEqual(r.returncode, ident.ABSENT)
        self.assertEqual(r.stdout.strip(), "", "resolve must print nothing when there is nothing")

    # ── case 2: exactly one -> reuse it ───────────────────────────────────────────────
    def test_single_identity_is_reused(self):
        p = self._file({TOK_A: "multiagency", TOK_B: "other"})
        r = self._cli("resolve", "multiagency", p)
        self.assertEqual(r.returncode, ident.OK)
        self.assertEqual(r.stdout.strip(), TOK_A)

    # ── case 3: several -> fail closed, name the count, never a token ─────────────────
    def test_multiple_identities_fail_closed_without_choosing(self):
        p = self._file({TOK_A: "multiagency", TOK_B: "multiagency", TOK_C: "multiagency"})
        r = self._cli("resolve", "multiagency", p)
        self.assertEqual(r.returncode, ident.AMBIGUOUS)
        self.assertEqual(r.stdout.strip(), "", "an ambiguous state must not yield a token")
        self.assertIn("3", r.stderr)
        for t in (TOK_A, TOK_B, TOK_C):
            self.assertNotIn(t, r.stderr, "a diagnostic leaked a credential")

    # ── case 4: THE REGRESSION — no journal, but authority exists -> reuse ────────────
    def test_identity_created_outside_provisioning_is_still_authoritative(self):
        """`seed-real.sh` registers an identity and writes no provisioning journal. Identity
        state must still report it, or provisioning mints a duplicate."""
        p = self._file({TOK_A: "multiagency"})
        self.assertEqual(self._cli("count", "multiagency", p).stdout.strip(), "1")
        self.assertEqual(self._cli("resolve", "multiagency", p).stdout.strip(), TOK_A)

    # ── case 5: journal may claim registered; state is what decides ───────────────────
    def test_state_reports_absence_even_when_a_journal_would_claim_otherwise(self):
        """The counterpart of case 4. This module has no notion of a journal at all — which is
        the point: it cannot be talked into reporting authority that is not in the map."""
        p = self._file({})
        self.assertEqual(self._cli("count", "multiagency", p).stdout.strip(), "0")
        self.assertEqual(self._cli("resolve", "multiagency", p).returncode, ident.ABSENT)

    # ── a corrupt map is not an empty one ─────────────────────────────────────────────
    def test_corrupt_map_refuses_rather_than_reading_as_empty(self):
        d = pathlib.Path(self._tmp.name) / "identities.json"
        d.write_text("{not json")
        with self.assertRaises(ident.IdentityStateError):
            ident.load(d)
        self.assertEqual(self._cli("count", "multiagency", d).returncode, ident.AMBIGUOUS)

    def test_missing_file_is_empty_not_an_error(self):
        self.assertEqual(ident.load(pathlib.Path(self._tmp.name) / "nope.json"), {})

    def test_count_never_prints_a_token(self):
        p = self._file({TOK_A: "multiagency", TOK_B: "multiagency"})
        r = self._cli("count", "multiagency", p)
        self.assertEqual(r.stdout.strip(), "2")
        self.assertNotIn(TOK_A, r.stdout + r.stderr)

    # ── the writes: one map, one writer ───────────────────────────────────────────────
    # These used to be four inline heredocs across three shell scripts. Only ONE of them
    # refused a corrupt map; the two removal paths did a bare json.load, and they did not agree
    # on the output bytes. Each case below is one of those disagreements, pinned.

    def test_add_registers_without_disturbing_other_orgs(self):
        p = self._file({TOK_A: "other"})
        ident.add(TOK_B, "multiagency", p)
        self.assertEqual(ident.load(p), {TOK_A: "other", TOK_B: "multiagency"})

    def test_a_corrupt_map_refuses_every_mutation_and_changes_nothing(self):
        """THE INVARIANT. Rewriting an unparseable map from a default-empty reading REVOKES every
        other client's org token — the file is hot-reloaded, so they all 401 at once. Both
        removal paths could do exactly that before this module owned the writes."""
        p = pathlib.Path(self._tmp.name) / "identities.json"
        p.write_text("{ this is not json")
        for op in (lambda: ident.add(TOK_A, "multiagency", p),
                   lambda: ident.remove_org("multiagency", p),
                   lambda: ident.other_org_token("multiagency", p)):
            with self.assertRaises(ident.IdentityStateError):
                op()
        self.assertEqual(p.read_text(), "{ this is not json", "the map was rewritten")

    def test_remove_is_idempotent_and_reports_what_it_did(self):
        """Zero removed is success, not failure: the provisioning compensator and deprovisioning
        both run against maps that may never have held this org."""
        p = self._file({TOK_A: "multiagency", TOK_B: "multiagency", TOK_C: "other"})
        self.assertEqual(ident.remove_org("multiagency", p), 2)
        self.assertEqual(ident.load(p), {TOK_C: "other"})
        self.assertEqual(ident.remove_org("multiagency", p), 0)
        self.assertEqual(ident.remove_org("never-existed", p), 0)
        self.assertEqual(ident.load(p), {TOK_C: "other"})

    def test_removing_from_an_absent_map_is_zero_not_a_crash(self):
        self.assertEqual(ident.remove_org("x", pathlib.Path(self._tmp.name) / "nope.json"), 0)

    def test_a_written_map_is_private_before_it_has_content(self):
        p = pathlib.Path(self._tmp.name) / "sub" / "identities.json"
        ident.add(TOK_A, "multiagency", p)          # parent does not exist yet
        self.assertEqual(p.stat().st_mode & 0o777, 0o600)

    def test_other_finds_a_different_orgs_token_and_none_when_alone(self):
        p = self._file({TOK_A: "multiagency"})
        self.assertIsNone(ident.other_org_token("multiagency", p),
                          "the first tenant has no other org to cross-check against")
        ident.add(TOK_B, "other", p)
        self.assertEqual(ident.other_org_token("multiagency", p), TOK_B)

    def test_add_takes_the_token_from_the_environment_never_argv(self):
        """A token on argv is visible in `ps` to every user on the box."""
        p = self._file({})
        r = subprocess.run([sys.executable, str(HERE / "identities.py"), "add", "multiagency"],
                           capture_output=True, text=True,
                           env={"ACCOUNT_IDENTITIES_FILE": str(p), "PATH": "/usr/bin:/bin"})
        self.assertEqual(r.returncode, ident.USAGE)
        self.assertIn("ORG_TOKEN", r.stderr)
        r = subprocess.run([sys.executable, str(HERE / "identities.py"), "add", "multiagency"],
                           capture_output=True, text=True,
                           env={"ACCOUNT_IDENTITIES_FILE": str(p), "ORG_TOKEN": TOK_A,
                                "PATH": "/usr/bin:/bin"})
        self.assertEqual(r.returncode, ident.OK)
        self.assertNotIn(TOK_A, r.stdout + r.stderr, "the CLI echoed the token it stored")
        self.assertEqual(ident.load(p), {TOK_A: "multiagency"})


if __name__ == "__main__":
    unittest.main(verbosity=2)
