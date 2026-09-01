#!/usr/bin/env python3
"""Provisioning's last gate must ask about the registry the bridge reads, not about one file.

THE DEFECT THIS PINS. `provision.sh`'s smoke leg 5 validated the staged entry ALONE in a temp
directory. Every cross-entry rule in `registry.load_clients` — `seen_accounts`, `seen_tokens`,
`seen_groups`, duplicate slug — compares entries against each other, so one entry satisfies all
of them vacuously. `provision.sh` step 1 REUSES an org's existing Account-Service credential
when the identity map holds exactly one, so a second slug on an existing org passed the leg,
activated by the atomic `mv`, and then made `registry.py`'s D-091 rule refuse the WHOLE registry
at the next bridge start — every tenant, not just the new one.

`test_the_old_single_entry_shape_did_not_catch_it` is the mutation proof: it reproduces the old
behaviour on the same input and asserts it passes. If that test ever fails, the collision stopped
being reachable and this whole file can be re-argued. Every other test here would pass against
the old implementation too, which is exactly why that one is in the file.

Offline, stdlib-only, no instance and no Account Service. Fixtures are the committed synthetic
proof clients — never the internal candidates.

Run:  python3 deploy/lib/test_registry_validation.py
"""
import os
import pathlib
import sys
import tempfile
import unittest

HERE = pathlib.Path(__file__).resolve().parent
ROOT = HERE.parents[1]
FIXTURES = ROOT / "multi" / "verify" / "fixtures" / "clients"
sys.path.insert(0, str(HERE))

import registry_validation as rv  # noqa: E402  path set above

# Distinct, obviously-synthetic values. `load_clients` rejects a file whose two credentials are
# equal, so every pair here differs even where the test does not care.
A_ACCOUNT, A_MEMBER = "acct-aaaa1111", "memb-aaaa2222"
B_ACCOUNT, B_MEMBER = "acct-bbbb3333", "memb-bbbb4444"


def _fill(slug, account_token, member_token, group=None, org=None):
    """The committed template with the two REQUIRED placeholders filled, as an operator does."""
    out = []
    for line in (FIXTURES / f"{slug}.env.template").read_text().splitlines():
        key = line.split("=", 1)[0].strip()
        if key == "ACCOUNT_TOKEN":
            line = f"ACCOUNT_TOKEN={account_token}"
        elif key == "IRONCLAW_TOKEN":
            line = f"IRONCLAW_TOKEN={member_token}"
        elif key == "TELEGRAM_GROUP_ID" and group is not None:
            line = f'TELEGRAM_GROUP_ID="{group}"'
        elif key == "ORG_ID" and org is not None:
            line = f"ORG_ID={org}"
        out.append(line)
    return "\n".join(out) + "\n"


def _write_tenant(directory, slug, env_text, guidance=True):
    directory = pathlib.Path(directory)
    directory.mkdir(parents=True, exist_ok=True)
    (directory / f"{slug}.env").write_text(env_text)
    if guidance:
        (directory / f"{slug}.guidance.md").write_text(
            (FIXTURES / f"{slug}.guidance.md").read_text())


class RegistryValidation(unittest.TestCase):
    def setUp(self):
        # A stray operator token or off-pin MODEL in the developer's shell changes what
        # `load_clients` does. Pin the environment so every verdict here is about the fixtures.
        self._saved = {k: os.environ.pop(k, None)
                       for k in ("MODEL", "CLIENTS_DIR", "IRONCLAW_OPERATOR_TOKEN",
                                 "IRONCLAW_REBORN_WEBUI_TOKEN", "WEBUI_TOKEN")}
        self._tmp = tempfile.TemporaryDirectory()
        self.root = pathlib.Path(self._tmp.name)
        self.live = self.root / "clients"
        self.staging = self.live / ".staging"
        self.live.mkdir()
        self.staging.mkdir()

    def tearDown(self):
        self._tmp.cleanup()
        for k, v in self._saved.items():
            if v is not None:
                os.environ[k] = v

    # ── the regression ────────────────────────────────────────────────────────────────

    def test_a_staged_tenant_reusing_a_live_account_token_is_refused(self):
        """The exact shape provisioning's credential-reuse branch produces."""
        _write_tenant(self.live, "proof-a", _fill("proof-a", A_ACCOUNT, A_MEMBER))
        # proof-b, provisioned onto proof-a's ORG: step 1 hands it proof-a's existing credential.
        _write_tenant(self.staging, "proof-b",
                      _fill("proof-b", A_ACCOUNT, B_MEMBER), guidance=False)
        _write_tenant(self.live, "proof-b", _fill("proof-b", A_ACCOUNT, B_MEMBER))
        (self.live / "proof-b.env").unlink()   # guidance is live; only the entry is staged

        verdict, detail = rv.validate(self.live, self.staging / "proof-b.env",
                                      self.live / "proof-b.guidance.md")
        self.assertEqual(rv.STAGED_CONFLICT, verdict, detail)
        self.assertIn("ACCOUNT_TOKEN", detail)
        self.assertIn("proof-a", detail, "the refusal must name the tenant already holding it")
        # The loader names the offending FILE, and that is most of the message's value. Every
        # path in it points into a mirror directory that is deleted before anyone reads it, so
        # assert the operator is handed a path that still exists.
        named = [w for w in detail.replace(":", " ").split() if w.endswith(".env")]
        self.assertTrue(named, f"no .env path named in the refusal: {detail}")
        for path in named:
            self.assertTrue(pathlib.Path(path).is_file(),
                            f"the refusal names {path}, which the operator cannot open")

    def test_the_old_single_entry_shape_did_not_catch_it(self):
        """MUTATION PROOF — the old leg passes the input the test above refuses.

        This is what makes the regression test meaningful rather than decorative: without it,
        every assertion in this file would also pass against the implementation that shipped the
        defect.
        """
        _write_tenant(self.live, "proof-a", _fill("proof-a", A_ACCOUNT, A_MEMBER))
        _write_tenant(self.staging, "proof-b",
                      _fill("proof-b", A_ACCOUNT, B_MEMBER), guidance=False)
        _write_tenant(self.live, "proof-b", _fill("proof-b", A_ACCOUNT, B_MEMBER))
        (self.live / "proof-b.env").unlink()

        # The old shape: the staged entry and its guidance, and nothing else.
        with tempfile.TemporaryDirectory() as alone:
            _write_tenant(alone, "proof-b", (self.staging / "proof-b.env").read_text())
            clients = rv._load_clients()(alone)
        self.assertEqual({"proof-b"}, set(clients),
                         "the old single-entry leg accepted a reused ACCOUNT_TOKEN")

    # ── positive controls: an empty registry passes every cross-entry rule ────────────

    def test_distinct_credentials_are_admitted(self):
        _write_tenant(self.live, "proof-a", _fill("proof-a", A_ACCOUNT, A_MEMBER))
        _write_tenant(self.staging, "proof-b",
                      _fill("proof-b", B_ACCOUNT, B_MEMBER), guidance=False)
        _write_tenant(self.live, "proof-b", _fill("proof-b", B_ACCOUNT, B_MEMBER))
        (self.live / "proof-b.env").unlink()

        verdict, detail = rv.validate(self.live, self.staging / "proof-b.env",
                                      self.live / "proof-b.guidance.md")
        self.assertEqual(rv.OK, verdict, detail)
        self.assertIn("proof-b", detail)
        self.assertIn("1 live tenant", detail, "the live entry must actually have been loaded")

    def test_two_tokens_for_one_org_remain_permitted(self):
        """ORG_ID is operator metadata. Two credentials for one org is a rotation in flight,
        not a collision — the rule is about the credential, and over-tightening it here would
        break the supported rotation path."""
        _write_tenant(self.live, "proof-a", _fill("proof-a", A_ACCOUNT, A_MEMBER, org="shared"))
        _write_tenant(self.staging, "proof-b",
                      _fill("proof-b", B_ACCOUNT, B_MEMBER, org="shared"), guidance=False)
        _write_tenant(self.live, "proof-b",
                      _fill("proof-b", B_ACCOUNT, B_MEMBER, org="shared"))
        (self.live / "proof-b.env").unlink()

        verdict, detail = rv.validate(self.live, self.staging / "proof-b.env",
                                      self.live / "proof-b.guidance.md")
        self.assertEqual(rv.OK, verdict, detail)

    def test_the_first_tenant_on_an_empty_host_is_admitted(self):
        _write_tenant(self.staging, "proof-a",
                      _fill("proof-a", A_ACCOUNT, A_MEMBER), guidance=False)
        _write_tenant(self.live, "proof-a", _fill("proof-a", A_ACCOUNT, A_MEMBER))
        (self.live / "proof-a.env").unlink()

        verdict, detail = rv.validate(self.live, self.staging / "proof-a.env",
                                      self.live / "proof-a.guidance.md")
        self.assertEqual(rv.OK, verdict, detail)

    # ── the other cross-entry rules the single-entry shape could not reach ───────────

    def test_a_staged_tenant_reusing_a_live_member_token_is_refused(self):
        _write_tenant(self.live, "proof-a", _fill("proof-a", A_ACCOUNT, A_MEMBER))
        _write_tenant(self.staging, "proof-b",
                      _fill("proof-b", B_ACCOUNT, A_MEMBER), guidance=False)
        _write_tenant(self.live, "proof-b", _fill("proof-b", B_ACCOUNT, A_MEMBER))
        (self.live / "proof-b.env").unlink()

        verdict, detail = rv.validate(self.live, self.staging / "proof-b.env",
                                      self.live / "proof-b.guidance.md")
        self.assertEqual(rv.STAGED_CONFLICT, verdict, detail)
        self.assertIn("IRONCLAW_TOKEN", detail)

    def test_a_staged_tenant_reusing_a_live_group_id_is_refused(self):
        _write_tenant(self.live, "proof-a", _fill("proof-a", A_ACCOUNT, A_MEMBER))
        group = "-100900011"   # proof-a's committed synthetic id
        _write_tenant(self.staging, "proof-b",
                      _fill("proof-b", B_ACCOUNT, B_MEMBER, group=group), guidance=False)
        _write_tenant(self.live, "proof-b",
                      _fill("proof-b", B_ACCOUNT, B_MEMBER, group=group))
        (self.live / "proof-b.env").unlink()

        verdict, detail = rv.validate(self.live, self.staging / "proof-b.env",
                                      self.live / "proof-b.guidance.md")
        self.assertEqual(rv.STAGED_CONFLICT, verdict, detail)
        self.assertIn("TELEGRAM_GROUP_ID", detail)

    # ── "no" has two meanings, and only one of them is this tenant's fault ───────────

    def test_a_registry_already_broken_is_not_blamed_on_the_staged_tenant(self):
        """Two LIVE tenants already share a credential. The staged tenant is uninvolved, and
        reporting STAGED_CONFLICT here would send the operator to tear down a good tenant."""
        _write_tenant(self.live, "proof-a", _fill("proof-a", A_ACCOUNT, A_MEMBER))
        _write_tenant(self.live, "proof-b", _fill("proof-b", A_ACCOUNT, B_MEMBER))
        staged = self.root / "third.env"
        staged.write_text(_fill("proof-a", "acct-cccc5555", "memb-cccc6666"))

        verdict, detail = rv.validate(self.live, staged, self.live / "proof-a.guidance.md")
        self.assertEqual(rv.REGISTRY_INVALID, verdict, detail)

    def test_a_missing_staged_entry_is_a_usage_error_not_a_pass(self):
        verdict, detail = rv.validate(self.live, self.staging / "absent.env")
        self.assertEqual(rv.USAGE, verdict, detail)

    def test_live_only_mode_reports_the_registry_without_a_staged_entry(self):
        _write_tenant(self.live, "proof-a", _fill("proof-a", A_ACCOUNT, A_MEMBER))
        verdict, detail = rv.validate(self.live)
        self.assertEqual(rv.OK, verdict, detail)
        self.assertIn("1 live tenant", detail)

    # ── the property the module claims about credentials ─────────────────────────────

    def test_credentials_are_linked_never_copied(self):
        """A copy of a tenant's `.env` lands at the process umask. `_mirror` must symlink, so no
        second readable copy of a credential exists even for the life of the check."""
        _write_tenant(self.live, "proof-a", _fill("proof-a", A_ACCOUNT, A_MEMBER))
        with tempfile.TemporaryDirectory() as d:
            rv._mirror(d, self.live)
            entries = sorted(pathlib.Path(d).iterdir())
            self.assertTrue(entries, "the mirror produced nothing to check")
            for entry in entries:
                self.assertTrue(entry.is_symlink(), f"{entry.name} is a copy, not a link")
                # `is_symlink` alone would be satisfied by a link to a copy. The target must be
                # the operator's own file, so no second copy of the credential exists at all.
                # `.resolve()` on both sides: macOS reaches a temp dir through the /var ->
                # /private/var link, so the raw strings differ for the same file.
                self.assertEqual((self.live / entry.name).resolve(),
                                 pathlib.Path(os.readlink(entry)).resolve(),
                                 f"{entry.name} links somewhere other than the live registry")

    def test_exit_codes_are_distinct_and_stable(self):
        """The shell branches on these. A collision would make a broken registry read as a
        conflict this tenant introduced."""
        self.assertEqual(0, rv.EXIT[rv.OK])
        self.assertEqual(4, len(set(rv.EXIT.values())))
        self.assertNotIn(1, rv.EXIT.values(), "1 is a traceback, not a verdict")


if __name__ == "__main__":
    unittest.main(verbosity=2)
