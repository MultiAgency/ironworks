#!/usr/bin/env python3
"""Account Service guards — offline, no flask, no psycopg, no database.

Run: python3 deploy/account-intel/data/test_service_guards.py

These two rules were previously one line each inside a Flask handler, which meant nothing could
test them without the service's whole dependency stack. Both are security-relevant:

  * the health-failure body is what an UNAUTHENTICATED caller learns when the backend breaks;
  * the identity map is the only thing between a request and an org's rows.
"""
import pathlib
import re
import importlib.util
import json
import os
import sys
import tempfile
import types
import unittest
from unittest import mock

from service_guards import (IdentityMapError, duplicate_orgs, insecure_mode, like_contains,
                            new_ref, safe_error, validate_identity_map)

SERVICE_PY = pathlib.Path(__file__).resolve().parent / "service.py"


def load_service(identity_file):
    """Load the real service routes with only dependency construction replaced.

    The canonical development suite deliberately does not install Flask/Psycopg. These tiny
    module doubles keep that supply-chain boundary while exercising `service.py`'s actual reload
    state and `/ready` function; database readiness itself is supplied as an already-ready schema
    because its real dict_row path is covered by `test_migrations.py` and the scheduled smoke.
    """
    flask = types.ModuleType("flask")

    class FakeFlask:
        def __init__(self, *_args, **_kwargs):
            pass

        def get(self, *_args, **_kwargs):
            return lambda function: function

    flask.Flask = FakeFlask
    flask.request = types.SimpleNamespace(headers={}, args={})
    flask.jsonify = lambda body: body
    psycopg = types.ModuleType("psycopg")
    rows = types.ModuleType("psycopg.rows")
    rows.dict_row = object()
    pool = types.ModuleType("psycopg_pool")
    pool.ConnectionPool = lambda *_args, **_kwargs: object()

    name = f"account_service_reload_test_{id(identity_file)}"
    spec = importlib.util.spec_from_file_location(name, SERVICE_PY)
    module = importlib.util.module_from_spec(spec)
    env = {"ACCOUNT_DB_DSN": "postgresql://unused/review",
           "ACCOUNT_IDENTITIES": "{}", "ACCOUNT_IDENTITIES_FILE": str(identity_file)}
    with mock.patch.dict(os.environ, env, clear=False), mock.patch.dict(
            sys.modules, {"flask": flask, "psycopg": psycopg, "psycopg.rows": rows,
                          "psycopg_pool": pool}):
        spec.loader.exec_module(module)

    class Connection:
        def __enter__(self):
            return object()

        def __exit__(self, *_args):
            return False

    module._conn = Connection
    module.migration_status = lambda _conn: {
        "ready": True, "problems": [],
        "expected": [{"version": "001"}, {"version": "002"}, {"version": "003"}],
        "applied": {"001": {}, "002": {}, "003": {}},
    }
    return module


class HealthErrors(unittest.TestCase):
    def test_error_body_carries_no_detail(self):
        body, status = safe_error()
        self.assertEqual(status, 500)
        self.assertEqual(set(body), {"ok", "error", "ref"})
        self.assertIs(body["ok"], False)
        self.assertEqual(body["error"], "backend_unavailable")

    def test_the_code_is_stable_and_the_ref_is_not(self):
        """A caller branches on `error`; an operator greps on `ref`. Swap those properties and
        the field is useless for both."""
        a, _ = safe_error()
        b, _ = safe_error()
        self.assertEqual(a["error"], b["error"])
        self.assertNotEqual(a["ref"], b["ref"])
        self.assertEqual(len(a["ref"]), 12)
        self.assertTrue(re.fullmatch(r"[0-9a-f]{12}", a["ref"]))

    def test_a_supplied_ref_is_used_verbatim(self):
        """The log line and the response must name the SAME id, or the correlation is fiction."""
        ref = new_ref()
        body, _ = safe_error("backend_unavailable", ref)
        self.assertEqual(body["ref"], ref)

    def test_the_health_route_does_not_serialize_an_exception(self):
        """Regression guard on the actual route: the leak was `jsonify({... str(e) ...})`, and
        the fix is only a fix while nothing puts the exception back.

        Read as CODE, not text. The route's own docstring quotes `str(e)` while explaining why
        it is gone, and a substring check over the raw source cannot tell the explanation from
        the defect. Comments, docstrings and the `print()` calls — which never leave the
        process — are removed first; what is left is what runs."""
        import io
        import textwrap
        import tokenize
        src = SERVICE_PY.read_text()
        route = src.split('@app.get("/health")', 1)[1].split("@app.get(", 1)[0]
        route = "\n".join(ln for ln in route.splitlines() if "print(" not in ln)
        code = []
        for tok in tokenize.generate_tokens(io.StringIO(textwrap.dedent(route)).readline):
            if tok.type not in (tokenize.COMMENT, tokenize.STRING):
                code.append(tok.string)
        code = " ".join(code).replace(" ", "")
        for banned in ("str(e)", "repr(e)", "e.args", "traceback"):
            self.assertNotIn(banned, code,
                             f"the /health response path renders {banned} — that is the leak")
        self.assertIn("safe_error", code, "the route stopped using the shared safe body")


class LikePattern(unittest.TestCase):
    """`/find_account` is a lookup, and its search language belongs to it, not to the caller."""

    def test_a_bare_percent_no_longer_matches_everything(self):
        """THE REGRESSION. `?query=%` built the pattern `%%%`, which matches every row — so a
        lookup that answers with a named account returned the org's whole book up to
        MAX_MATCHES. Own-org only, so not a cross-tenant leak, but not what the endpoint says."""
        self.assertEqual(like_contains("%"), "%\\%%")

    def test_an_underscore_is_a_literal_not_a_single_character_wildcard(self):
        """The subtler half: `_` reads like an ordinary character in a company name and matches
        any one character."""
        self.assertEqual(like_contains("a_b"), "%a\\_b%")

    def test_a_backslash_the_caller_typed_survives_as_a_backslash(self):
        """The escape character is escaped FIRST. Doing it last would make the backslashes this
        function introduces indistinguishable from the caller's, and `\\%` would then read as an
        escaped percent the caller never wrote."""
        self.assertEqual(like_contains("a\\b"), "%a\\\\b%")
        self.assertEqual(like_contains("100\\%"), "%100\\\\\\%%")

    def test_an_ordinary_query_is_unchanged_apart_from_its_anchors(self):
        """POSITIVE CONTROL. Escaping must not alter a search that contains no metacharacter, or
        every real lookup would have been quietly changed to fix a case none of them hit."""
        for term in ("acme", "tallow & finch", "northwind labs", "o'brien", "a-b.c"):
            self.assertEqual(like_contains(term), f"%{term}%", term)

    def test_the_query_still_matches_its_own_substring(self):
        """The function is only useful if the escaped pattern still finds the thing. Checked
        against Python's own translation of LIKE semantics rather than asserting a string."""
        import re
        for term, name, should in (("acme", "acme corp", True),
                                   ("%", "acme corp", False),
                                   ("_", "acme corp", False),
                                   ("e c", "acme corp", True)):
            pattern = like_contains(term)
            regex = ""
            i = 0
            while i < len(pattern):
                ch = pattern[i]
                if ch == "\\" and i + 1 < len(pattern):
                    regex += re.escape(pattern[i + 1]); i += 2; continue
                regex += {"%": ".*", "_": "."}.get(ch, re.escape(ch)); i += 1
            self.assertEqual(bool(re.fullmatch(regex, name)), should,
                             f"{term!r} vs {name!r} via {pattern!r}")

    def test_the_service_names_the_same_escape_character_in_SQL(self):
        """A pattern escaped with one character and interpreted with another is not escaped.
        Postgres defaults to backslash, but a rule that decides what a query MEANS does not live
        in a default."""
        sql = SERVICE_PY.read_text()
        self.assertIn("ESCAPE '\\\\'", sql,
                      "find_account does not name the LIKE escape character explicitly")
        self.assertIn("like_contains(", sql, "find_account no longer escapes the query")
        self.assertNotIn('f"%{q.lower()}%"', sql,
                         "find_account still interpolates the raw query into a LIKE pattern")


class IdentityMap(unittest.TestCase):
    def test_a_good_map_passes_through(self):
        m = {"tok-a": "org-a", "tok-b": "org-b"}
        self.assertIs(validate_identity_map(m), m)

    def test_a_non_object_is_refused(self):
        for bad in ([], "string", 7, None):
            with self.assertRaises(IdentityMapError):
                validate_identity_map(bad, "identities.json")

    def test_a_non_string_org_is_refused(self):
        """The shape that used to reach a bound SQL parameter: the value is not an org id."""
        for bad in ({"tok": {"org": "a"}}, {"tok": None}, {"tok": ["a"]}, {"tok": 1}):
            with self.assertRaises(IdentityMapError):
                validate_identity_map(bad, "identities.json")

    def test_empty_strings_are_refused_on_both_sides(self):
        for bad in ({"": "org"}, {"   ": "org"}, {"tok": ""}, {"tok": "  "}):
            with self.assertRaises(IdentityMapError):
                validate_identity_map(bad, "identities.json")

    def test_the_error_names_the_file(self):
        try:
            validate_identity_map([], "/tmp/identities.json")
        except IdentityMapError as e:
            self.assertIn("/tmp/identities.json", str(e))
        else:
            self.fail("no error raised")

    def test_duplicate_orgs_are_reported_not_refused(self):
        """Two tokens for one org is a rotation in flight or a failed re-provision. Serving
        must continue — but the second token is authority nobody is tracking."""
        m = {"t1": "acme", "t2": "acme", "t3": "beta"}
        self.assertIs(validate_identity_map(m), m)
        self.assertEqual(duplicate_orgs(m), ["acme"])
        self.assertEqual(duplicate_orgs({"t1": "acme", "t2": "beta"}), [])


class IdentityReloadReadiness(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.path = pathlib.Path(self._tmp.name) / "identities.json"
        self.path.write_text(json.dumps({"current-token": "org-a"}))
        self.service = load_service(self.path)

    def tearDown(self):
        self._tmp.cleanup()

    def ready(self):
        result = self.service.ready()
        return result if isinstance(result, tuple) else (result, 200)

    def assert_ready(self):
        body, status = self.ready()
        self.assertEqual(status, 200, body)
        self.assertTrue(body["identity_ready"], body)

    def assert_not_ready(self, reason):
        body, status = self.ready()
        self.assertEqual(status, 503, body)
        self.assertFalse(body["identity_ready"], body)
        self.assertIn(reason, body["problems"])
        self.assertEqual(self.service._file_ident["map"], {},
                         "failed reload retained stale file-backed authority")

    def test_good_load_then_missing_source_is_503(self):
        self.assert_ready()
        self.path.unlink()
        self.assert_not_ready("identity_file_missing")

    def test_good_load_then_malformed_source_is_503(self):
        self.assert_ready()
        self.path.write_text("{not-json")
        self.assert_not_ready("identity_file_invalid")

    def test_failed_reload_then_corrected_source_recovers_to_200(self):
        self.assert_ready()
        self.path.write_text("[]")
        self.assert_not_ready("identity_file_invalid")
        self.path.write_text(json.dumps({"replacement-token": "org-a"}))
        self.assert_ready()
        self.assertEqual(self.service._file_ident["map"], {"replacement-token": "org-a"})


class FileMode(unittest.TestCase):
    def test_0600_is_clean(self):
        self.assertIsNone(insecure_mode(0o100600))

    def test_group_or_world_access_is_reported(self):
        for mode in (0o100640, 0o100604, 0o100666, 0o100660):
            self.assertIsNotNone(insecure_mode(mode), oct(mode))

    def test_the_reported_value_is_the_permission_bits_only(self):
        self.assertEqual(insecure_mode(0o100644), 0o644)


if __name__ == "__main__":
    unittest.main(verbosity=2)
