#!/usr/bin/env python3
"""Every Account Service statement is bound to the AUTHENTICATED org — offline.

Covers the readers and, since 2026-09-01, the one writer: `POST /append_activity`. The write
path is driven exactly like the reads — the real function, a recording cursor — because the
questions worth asking of it are the same two: which org got bound, and could the caller have
chosen it. What these tests deliberately do NOT establish is Postgres behaviour: the recorder
decides here what a unique index decides there, so create / replayed / conflict are proved as
ROUTE branches and remain unproven as database semantics until an operator deploys.

WHY THIS EXISTS. Cross-tenant isolation is the product's core invariant, and until now it had no
offline backing at all. It was proved by `multi/verify/test_two_clients.py` and
`test_adversarial_cross_org.py` (a live instance, credentials, a model), and by shell smoke in
`dev-up.sh` / `prod-up.sh` (Postgres and a running service). Every one of those needs an
environment CI does not have, so the assertion nothing could check was the one that matters most:
that a query cannot read another organization's rows.

`test_service_guards.py` next door covers the pure helpers — error shapes, LIKE escaping, the
identity map — and mentions `org_id` zero times. This file covers the routes.

WHAT IT ASSERTS, AND WHY IT IS NOT A GREP. It would be easy, and nearly worthless, to regex
`service.py` for `org_id = %s`. That passes on a query which names the column and then binds the
WRONG VALUE — a caller-supplied one, say — which is precisely the bug worth catching. So this
drives the REAL route functions against a recording cursor and asserts on what they actually
executed:

  1. every statement touching an org-scoped table names `org_id` in its predicate, AND
  2. the value BOUND for it is the org `_auth_org` resolved from the credential, AND
  3. that stays true when the caller asserts a different org in a header.

(2) is the one a source grep cannot make. (3) is the header-spoofing case that previously existed
only in `dev-up.sh`'s live smoke.

THE POSITIVE CONTROL IS LOAD-BEARING. "Every statement binds the org" is trivially true of a
route that executed nothing — and a fake that silently returned no rows would produce exactly
that. So each route asserts it recorded the number of statements it should have, and the suite
refuses to pass on an empty recording.

Flask and psycopg are module doubles for the same reason `test_service_guards.py` uses them: the
canonical dev environment deliberately does not install the service's runtime dependencies, and
that supply-chain boundary is worth more than the convenience of importing them here.

Run:  python3 deploy/account-intel/data/test_org_scoping.py
"""
import datetime
import importlib.util
import os
import pathlib
import sys
import types
import unittest
from unittest import mock

HERE = pathlib.Path(__file__).resolve().parent
SERVICE_PY = HERE / "service.py"

# The tables whose rows belong to exactly one organization. `schema.sql` keys all three by
# `(org_id, <id>)`; a statement reading any of them without the org bound is a cross-tenant read.
ORG_SCOPED_TABLES = ("accounts", "contacts", "activities")

TOKEN, ORG = "tok-authenticated", "org-authenticated"
OTHER_ORG = "org-somebody-else"


class Recorder:
    """A cursor that answers plausibly and remembers every statement and its bound parameters."""

    def __init__(self):
        self.statements = []

    def execute(self, sql, params=()):
        self.statements.append((" ".join(sql.split()), tuple(params)))
        return self

    # `updated_at`/`occurred_at` are None so the routes' `.isoformat()` post-processing is
    # skipped: this suite is about the WHERE clause, not about date rendering.
    def fetchall(self):
        return [{"account_id": "A-1", "name": "Fixture", "domain": "f.example",
                 "updated_at": None, "occurred_at": None, "contact_id": "C-1",
                 "activity_id": "V-1", "title": None, "engaged": None, "notes": None,
                 "kind": None, "body": None}]

    def fetchone(self):
        return {"account_id": "A-1", "name": "Fixture", "updated_at": None, "facts": None}


def load_service(recorder, headers, args, json_body=None):
    """The real `service.py`, with only its dependencies and connection replaced."""
    flask = types.ModuleType("flask")

    class FakeFlask:
        def __init__(self, *_a, **_k):
            pass

        def get(self, *_a, **_k):
            return lambda function: function

        # The append route is a POST; see the note in `test_service_guards.py`.
        def post(self, *_a, **_k):
            return lambda function: function

    flask.Flask = FakeFlask
    flask.request = types.SimpleNamespace(headers=headers, args=args,
                                      get_json=lambda **_k: json_body)
    flask.jsonify = lambda body: body
    psycopg = types.ModuleType("psycopg")
    rows = types.ModuleType("psycopg.rows")
    rows.dict_row = object()
    pool = types.ModuleType("psycopg_pool")
    pool.ConnectionPool = lambda *_a, **_k: object()

    name = f"account_service_orgscope_{id(recorder)}"
    spec = importlib.util.spec_from_file_location(name, SERVICE_PY)
    module = importlib.util.module_from_spec(spec)
    env = {"ACCOUNT_DB_DSN": "postgresql://unused/orgscope",
           # The one credential this suite authenticates with. Identity implies org, so this map
           # IS the server-side resolution the routes depend on.
           "ACCOUNT_IDENTITIES": f'{{"{TOKEN}": "{ORG}"}}',
           # A SEPARATE map for append authority. The read token above is deliberately absent
           # from it: that absence is what makes a reader unable to write, and it is asserted.
           "ACCOUNT_APPEND_IDENTITIES": f'{{"tok-append": "{ORG}"}}'}
    with mock.patch.dict(os.environ, env, clear=False), mock.patch.dict(
            sys.modules, {"flask": flask, "psycopg": psycopg, "psycopg.rows": rows,
                          "psycopg_pool": pool}):
        spec.loader.exec_module(module)

    class Connection:
        def __enter__(self_inner):
            return recorder

        def __exit__(self_inner, *_a):
            return False

    module._conn = Connection
    return module


def drive(route, args, headers=None):
    """Call one real route and return (response, recorded statements)."""
    recorder = Recorder()
    module = load_service(recorder, dict(headers or {"X-Service-Token": TOKEN}), dict(args))
    return getattr(module, route)(), recorder.statements


# ── the append boundary ─────────────────────────────────────────────────────────────────────
#
# The one route that writes. Driven the same way as the readers — the real function, a recording
# cursor — because the questions worth asking about it are the same ones: which org got bound,
# and could a caller have chosen it.
#
# WHAT THESE TESTS DO NOT ESTABLISH. `ON CONFLICT (org_id, activity_id) DO NOTHING` is Postgres
# behaviour against a real unique index. The recorder decides here what the database decides
# there, so these prove the ROUTE's branches — created / replayed / conflict — and say nothing
# about the index enforcing them. That remains unproven until an operator deploys.

APPEND_TOKEN = "tok-append"

#: The append credential resolves the SAME org as the read token, which is the case the seam
#: requires and checks. A second org exists below only to prove a caller cannot select it.
APPEND_ORG = ORG


def append_payload(**overrides):
    payload = {"activity_id": "share-abc123", "account_id": "A-1", "occurred_at": "2026-09-01",
               "body": "Customer requested a September renewal review.", "contributor": "alice",
               "expected_org": APPEND_ORG}
    payload.update(overrides)
    return payload


class AppendRecorder(Recorder):
    """A recording cursor that also decides what the database would have answered.

    `insert_wins` is the `ON CONFLICT DO NOTHING RETURNING` outcome, and `existing` is the row a
    losing insert then reads back. Both are the test's to choose precisely because Postgres is
    not here to choose them.
    """

    def __init__(self, *, account_exists=True, insert_wins=True, existing=None):
        super().__init__()
        self.account_exists = account_exists
        self.insert_wins = insert_wins
        self.existing = existing

    def fetchone(self):
        sql = self.statements[-1][0]
        if "FROM accounts" in sql:
            return {"account_id": "A-1"} if self.account_exists else None
        if sql.startswith("INSERT INTO activities"):
            return {"activity_id": "share-abc123"} if self.insert_wins else None
        if "FROM activities" in sql:
            return self.existing
        raise AssertionError(f"unexpected fetchone for: {sql}")


def drive_append(payload, *, token=APPEND_TOKEN, recorder=None, headers=None):
    recorder = recorder or AppendRecorder()
    module = load_service(recorder, dict(headers or {"X-Service-Token": token}), {},
                          json_body=payload)
    return module.append_activity(), recorder.statements


class OrgScoping(unittest.TestCase):
    def assert_every_statement_is_org_bound(self, statements, expected_count, org=ORG):
        # THE POSITIVE CONTROL. Without this the loop below is vacuously true of [].
        self.assertEqual(expected_count, len(statements),
                         f"expected {expected_count} statement(s), recorded {len(statements)} — "
                         "the route did not run, and 'every statement is org-bound' would pass "
                         "over nothing")
        for sql, params in statements:
            table = next((t for t in ORG_SCOPED_TABLES if f"FROM {t}" in sql), None)
            self.assertIsNotNone(table, f"statement reads no org-scoped table: {sql}")
            self.assertIn("org_id = %s", sql,
                          f"a {table} query has no org_id predicate: {sql}")
            # The half a source grep cannot make: the column is named AND the authenticated org
            # is what got bound to it.
            self.assertIn(org, params,
                          f"a {table} query names org_id but binds {params} — the predicate is "
                          "there and the value is not the authenticated org")

    def test_find_account_binds_the_authenticated_org(self):
        body, statements = drive("find_account", {"query": "fixture"})
        self.assert_every_statement_is_org_bound(statements, 1)
        self.assertEqual(ORG, body["org"])

    def test_list_accounts_binds_the_authenticated_org(self):
        body, statements = drive("list_accounts", {})
        self.assert_every_statement_is_org_bound(statements, 1)
        self.assertEqual(ORG, body["org"])

    def test_get_account_context_binds_the_org_on_all_three_reads(self):
        """accounts, contacts AND activities. The child reads are the easy ones to forget, and
        `schema.sql`'s composite foreign keys are the only other thing standing behind them."""
        body, statements = drive("get_account_context", {"account_id": "A-1"})
        self.assert_every_statement_is_org_bound(statements, 3)
        self.assertEqual({"accounts", "contacts", "activities"},
                         {t for sql, _ in statements for t in ORG_SCOPED_TABLES
                          if f"FROM {t}" in sql})
        self.assertEqual(ORG, body["org"])

    def test_a_caller_asserted_org_header_changes_nothing(self):
        """`X-Org-Id` is read nowhere in service.py — identity implies org. This pins that from
        the outside: the bound value must still be the token's org, on every statement.

        Previously proved only by `dev-up.sh`'s live smoke, which needs Postgres and a running
        service."""
        for route, args, count in (("find_account", {"query": "f"}, 1),
                                   ("list_accounts", {}, 1),
                                   ("get_account_context", {"account_id": "A-1"}, 3)):
            with self.subTest(route=route):
                body, statements = drive(route, args, headers={
                    "X-Service-Token": TOKEN, "X-Org-Id": OTHER_ORG})
                self.assert_every_statement_is_org_bound(statements, count)
                for _sql, params in statements:
                    self.assertNotIn(OTHER_ORG, params,
                                     "a caller-asserted org reached a query parameter")
                self.assertEqual(ORG, body["org"])

    def test_an_unknown_credential_executes_nothing(self):
        """401 before any statement runs. A route that queried first and authorised after would
        still return 401 while having read another org's rows."""
        for route, args in (("find_account", {"query": "f"}), ("list_accounts", {}),
                            ("get_account_context", {"account_id": "A-1"})):
            with self.subTest(route=route):
                body, statements = drive(route, args,
                                         headers={"X-Service-Token": "not-a-real-token"})
                self.assertEqual([], statements,
                                 "an unauthenticated request reached the database")
                self.assertEqual(401, body[1])

    def test_find_account_binds_an_escaped_pattern_not_the_raw_query(self):
        """The LIKE pattern reaching the database must be the ESCAPED query, not the raw one.

        `test_service_guards.py` proves `like_contains` escapes correctly, and asserted that
        `find_account` uses it by looking for the literal `f"%{q.lower()}%"` NOT appearing in
        `service.py`. That guards one spelling of the defect. Measured: reintroduce the raw
        interpolation as `f'%{q.lower()}%'` — single quotes — while leaving a `like_contains(`
        call on the same line, and the source-text assertion passes while `?query=%` is once
        again a listing of every row the org has.

        Asserting on the BOUND VALUE has no spelling to evade. This is the same recorder the org
        checks use: both questions are "what did the route actually pass to the database".
        """
        _body, statements = drive("find_account", {"query": "100%_x"})
        self.assertEqual(1, len(statements), "find_account did not run")
        _sql, params = statements[0]
        pattern = next((p for p in params if isinstance(p, str) and p.startswith("%")), None)
        self.assertIsNotNone(pattern, f"no LIKE pattern among the bound parameters: {params}")
        self.assertNotEqual("%100%_x%", pattern,
                            "the raw query was interpolated into the LIKE pattern — `?query=%` "
                            "would match every row this org has")
        # The wildcards the CALLER supplied must arrive escaped; the two the service adds itself
        # to make it a contains-match must not be.
        self.assertEqual("%100\\%\\_x%", pattern,
                         f"caller wildcards are not escaped in the bound pattern: {pattern!r}")

    def test_the_scoped_table_list_matches_the_schema(self):
        """This suite is only as complete as `ORG_SCOPED_TABLES`. If a new org-keyed table is
        added to the schema and not here, every assertion above keeps passing while the new
        table's reads go unchecked."""
        schema = (HERE / "schema.sql").read_text()
        keyed = {t for t in ("organizations", "accounts", "contacts", "activities")
                 if "PRIMARY KEY (org_id, " in schema.split(f"CREATE TABLE IF NOT EXISTS {t}")[-1]
                 .split("CREATE TABLE")[0]}
        self.assertEqual(set(ORG_SCOPED_TABLES), keyed,
                         "schema.sql keys a different set of tables by (org_id, …) than this "
                         "suite checks — add it to ORG_SCOPED_TABLES")


class AppendAuthority(unittest.TestCase):
    """Who may append, and whose organization it lands in."""

    def test_a_read_token_cannot_append(self):
        # The whole point of the second map. The read credential is real, resolves an org, and
        # is refused here — indistinguishably from an unknown token, so a reader learns only
        # that it did not work.
        body, statements = drive_append(append_payload(), token=TOKEN)
        self.assertEqual(({"error": "unauthorized"}, 401), body)
        self.assertEqual([], statements, "an unauthorized append must touch the database")

    def test_an_unknown_token_cannot_append(self):
        body, statements = drive_append(append_payload(), token="tok-nobody")
        self.assertEqual(({"error": "unauthorized"}, 401), body)
        self.assertEqual([], statements)

    def test_no_append_configuration_authorizes_nobody(self):
        # A deployment that never sets the append map stays read-only, and does so without
        # failing to start — readiness and every reader are untouched.
        recorder = AppendRecorder()
        module = load_service(recorder, {"X-Service-Token": APPEND_TOKEN}, {},
                              json_body=append_payload())
        module.ENV_APPEND_IDENTITIES = {}
        module.APPEND_IDENTITIES_FILE = ""
        self.assertEqual(({"error": "unauthorized"}, 401), module.append_activity())
        self.assertEqual([], recorder.statements)

    def test_every_append_statement_binds_the_credential_resolved_org(self):
        _body, statements = drive_append(append_payload())
        self.assertEqual(2, len(statements),
                         "the account check and the insert must both have run")
        for sql, params in statements:
            self.assertIn(APPEND_ORG, params,
                          f"a write-path statement does not bind the resolved org: {sql}")

    def test_a_caller_cannot_select_the_organization(self):
        # `expected_org` is a guard, never a selector: a caller naming another org is refused
        # before any statement runs, and the org bound is always the token's.
        body, statements = drive_append(append_payload(expected_org=OTHER_ORG))
        self.assertEqual(409, body[1])
        self.assertEqual("org_mismatch", body[0]["error"])
        self.assertEqual(APPEND_ORG, body[0]["org"])
        self.assertEqual([], statements, "a mismatched org must be refused before any write")

    def test_an_org_header_changes_nothing(self):
        # The header-spoofing case the readers already cover, on the one route that writes.
        body, statements = drive_append(
            append_payload(), headers={"X-Service-Token": APPEND_TOKEN, "X-Org-Id": OTHER_ORG})
        self.assertEqual(201, body[1])
        self.assertEqual(APPEND_ORG, body[0]["org"])
        for _sql, params in statements:
            self.assertNotIn(OTHER_ORG, params)


class AppendIdempotency(unittest.TestCase):
    """Created, replayed, conflicted — and never updated."""

    def test_a_new_row_is_created(self):
        body, statements = drive_append(append_payload())
        self.assertEqual(201, body[1])
        self.assertEqual("created", body[0]["status"])
        self.assertEqual("share-abc123", body[0]["activity_id"])
        insert = next(s for s in statements if s[0].startswith("INSERT INTO activities"))
        self.assertIn("ON CONFLICT (org_id, activity_id) DO NOTHING", insert[0])
        self.assertNotIn("DO UPDATE", insert[0],
                         "conflict-update would let a replay rewrite confirmed evidence")
        # The kind is the server's, never the caller's.
        self.assertIn("shared-note", insert[1])

    def test_an_exact_replay_changes_nothing_and_reports_success(self):
        same = {"org_id": APPEND_ORG, "account_id": "A-1", "occurred_at": "2026-09-01",
                "kind": "shared-note",
                "body": "Customer requested a September renewal review.",
                "contributor": "alice"}
        body, statements = drive_append(
            append_payload(),
            recorder=AppendRecorder(insert_wins=False, existing=same))
        self.assertEqual(200, body[1])
        self.assertEqual("replayed", body[0]["status"])
        self.assertFalse([s for s in statements if s[0].startswith("UPDATE")],
                         "a replay must not write")

    def test_a_date_that_came_back_as_a_date_object_still_replays(self):
        # psycopg returns `occurred_at` as `datetime.date`; a comparison that only handled
        # strings would report every replay as a conflict against a real database.
        same = {"org_id": APPEND_ORG, "account_id": "A-1",
                "occurred_at": datetime.date(2026, 9, 1), "kind": "shared-note",
                "body": "Customer requested a September renewal review.",
                "contributor": "alice"}
        body, _ = drive_append(append_payload(),
                               recorder=AppendRecorder(insert_wins=False, existing=same))
        self.assertEqual(200, body[1], body)

    def test_any_differing_immutable_field_is_a_conflict_and_writes_nothing(self):
        base = {"org_id": APPEND_ORG, "account_id": "A-1", "occurred_at": "2026-09-01",
                "kind": "shared-note",
                "body": "Customer requested a September renewal review.",
                "contributor": "alice"}
        for field, value in (("account_id", "A-2"), ("occurred_at", "2026-09-02"),
                             ("kind", "note"), ("body", "something else entirely"),
                             ("contributor", "bob")):
            existing = dict(base, **{field: value})
            body, statements = drive_append(
                append_payload(),
                recorder=AppendRecorder(insert_wins=False, existing=existing))
            self.assertEqual(409, body[1], f"{field} should conflict")
            self.assertEqual("conflict", body[0]["error"])
            self.assertEqual(field, body[0]["differing_field"])
            self.assertFalse([s for s in statements if s[0].startswith("UPDATE")],
                             f"{field}: a conflict must change nothing")


class AppendValidation(unittest.TestCase):
    def test_an_account_this_org_does_not_have_is_refused(self):
        body, statements = drive_append(append_payload(),
                                        recorder=AppendRecorder(account_exists=False))
        self.assertEqual(404, body[1])
        self.assertEqual("unknown_account", body[0]["error"])
        self.assertEqual(1, len(statements), "only the account check should have run")

    def test_an_id_outside_the_shared_namespace_is_refused(self):
        # The namespace guard: this route may not touch an id the client's team authored.
        body, statements = drive_append(append_payload(activity_id="NW-INT-01"))
        self.assertEqual(400, body[1])
        self.assertEqual("activity_id_outside_shared_namespace", body[0]["error"])
        self.assertEqual([], statements)

    def test_malformed_fields_are_refused_before_any_statement(self):
        for overrides, code in (
            ({"contributor": "Alice"}, "invalid_contributor"),
            ({"contributor": ""}, "invalid_contributor"),
            ({"occurred_at": "2026-02-31"}, "invalid_occurred_at"),
            ({"occurred_at": "yesterday"}, "invalid_occurred_at"),
            ({"body": ""}, "invalid_body"),
            ({"body": "x" * 9000}, "invalid_body"),
            ({"account_id": ""}, "invalid_account_id"),
        ):
            body, statements = drive_append(append_payload(**overrides))
            self.assertEqual(400, body[1], overrides)
            self.assertEqual(code, body[0]["error"], overrides)
            self.assertEqual([], statements, overrides)

    def test_a_body_is_not_content_filtered(self):
        # The note is a person's own words. Length is bounded because a column is; nothing else
        # about it is this service's business.
        body, _ = drive_append(append_payload(
            body="Renewal at 40% discount -- see **the thread**; contact: a@b.example"))
        self.assertEqual(201, body[1])


class ContributorReadback(unittest.TestCase):
    def test_get_account_context_selects_contributor(self):
        # The other half of attribution: written by the append route, and returned by the one
        # read the seam uses. Without this an activity would carry a contributor nobody sees.
        _body, statements = drive("get_account_context", {"account_id": "A-1"})
        activities = next(s for s in statements if "FROM activities" in s[0])
        self.assertIn("contributor", activities[0])


if __name__ == "__main__":
    unittest.main(verbosity=2)
