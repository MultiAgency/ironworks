#!/usr/bin/env python3
"""Offline tests for migration metadata and schema-readiness classification."""
import pathlib
import tempfile
import unittest

import migrations


class Result:
    def __init__(self, rows):
        self.rows = rows

    def fetchone(self):
        return self.rows[0] if self.rows else None

    def fetchall(self):
        return self.rows


class FakeConnection:
    """A stand-in for psycopg, and the PK rows are the part to be careful about.

    A list here is truthful ONLY because the query casts `column_name::text`. Without that cast
    psycopg returns the literal string '{org_id,account_id}' for `sql_identifier[]`, and a fake
    that hands back a list models what the author expected rather than what the driver does —
    so every test below passed while `apply()` was raising on every real database. `PkShape`
    covers the driver's actual output; do not "simplify" these rows to strings, and do not drop
    the cast from the query without reading `_pk_columns`.
    """

    def __init__(self, *, tracked=True, current=True, checksum_override=None):
        self.tracked = tracked
        self.current = current
        self.checksum_override = checksum_override

    def execute(self, query, params=()):
        compact = " ".join(query.split())
        if "to_regclass" in compact:
            return Result([("ironworks_schema_migrations",)] if self.tracked else [(None,)])
        if "information_schema.table_constraints" in compact:
            if not self.current:
                return Result([("accounts", ["account_id"])])
            return Result([
                ("accounts", ["org_id", "account_id"]),
                ("contacts", ["org_id", "contact_id"]),
                ("activities", ["org_id", "activity_id"]),
            ])
        if "information_schema.columns" in compact:
            # Two tables are probed now, and the fake has to tell them apart: 004 lives on
            # `activities` while 002 and 003 live on `accounts`. Answering one column list for
            # both would let a migration signature pass by reading another table's shape.
            if "table_name = 'activities'" in compact:
                cols = ["activity_id", "org_id", "account_id", "body"]
                if self.current:
                    cols += ["contributor"]
            elif "table_name = 'accounts'" in compact:
                cols = ["account_id", "org_id"]
                if self.current:
                    cols += ["owner", "stage", "value_band", "facts"]
            else:
                raise AssertionError(f"unexpected column probe: {compact}")
            return Result([(c,) for c in cols])
        if "FROM ironworks_schema_migrations" in compact:
            rows = []
            for item in migrations.expected():
                checksum = self.checksum_override or item["checksum"]
                rows.append((item["version"], item["name"], checksum,
                             "2026-08-26T00:00:00+00:00"))
            return Result(rows)
        raise AssertionError(f"unexpected query: {compact}")


class DictRowConnection(FakeConnection):
    """The mapping row shape configured by the production Account Service pool."""

    def execute(self, query, params=()):
        result = super().execute(query, params)
        compact = " ".join(query.split())
        if "to_regclass" in compact:
            names = ("to_regclass",)
        elif "information_schema.table_constraints" in compact:
            names = ("table_name", "array_agg")
        elif "information_schema.columns" in compact:
            names = ("column_name",)
        elif "FROM ironworks_schema_migrations" in compact:
            names = ("version", "name", "checksum", "applied_at")
        else:
            raise AssertionError(f"unexpected query: {compact}")
        return Result([dict(zip(names, row, strict=True)) for row in result.rows])


class MigrationContract(unittest.TestCase):
    def test_committed_files_have_stable_unique_versions_and_checksums(self):
        expected = migrations.expected()
        self.assertEqual([x["version"] for x in expected], ["001", "002", "003", "004"])
        self.assertEqual(len({x["checksum"] for x in expected}), len(expected))
        self.assertTrue(all(len(x["checksum"]) == 64 for x in expected))

    def test_current_recorded_schema_is_ready(self):
        result = migrations.status(FakeConnection())
        self.assertTrue(result["ready"], result)
        self.assertEqual(result["problems"], [])

    def test_production_dict_rows_are_ready(self):
        result = migrations.status(DictRowConnection())
        self.assertTrue(result["ready"], result)
        self.assertEqual(result["problems"], [])

    def test_current_but_unrecorded_schema_requires_reconciliation(self):
        result = migrations.status(FakeConnection(tracked=False))
        self.assertFalse(result["ready"])
        self.assertEqual(len(result["problems"]), 4)
        self.assertTrue(all("not recorded" in p for p in result["problems"]))

    def test_recorded_but_legacy_schema_fails_readiness(self):
        result = migrations.status(FakeConnection(current=False))
        self.assertFalse(result["ready"])
        self.assertTrue(any("signature is absent" in p for p in result["problems"]))

    def test_changed_migration_file_is_detected(self):
        result = migrations.status(FakeConnection(checksum_override="0" * 64))
        self.assertFalse(result["ready"])
        self.assertEqual(sum("checksum differs" in p for p in result["problems"]), 4)

    def test_the_pk_query_casts_the_identifier_to_text(self):
        """The cast is the fix; this pins it at the query. `sql_identifier[]` has no psycopg
        array loader, so without `::text` the value arrives as a string and every signature
        check for migration 001 fails against a schema that is actually correct."""
        source = pathlib.Path(migrations.__file__).read_text()
        # assertTrue, not assertIn: a failing assertIn prints the whole module as the haystack.
        self.assertTrue("array_agg(kcu.column_name::text" in source,
                        "the primary-key query lost its ::text cast")

    def test_runner_strips_only_transaction_wrapper(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = pathlib.Path(tmp) / "m.sql"
            path.write_text("BEGIN;\nCREATE TABLE x (id int);\nCOMMIT;\n")
            self.assertEqual(migrations._ddl(path).strip(), "CREATE TABLE x (id int);")


class PkShape(unittest.TestCase):
    """The shape psycopg actually returns, which the fake above cannot exhibit on its own.

    Every one of these fails against the code as originally shipped: `tuple('{org_id,...}')`
    walks the literal character by character, so migration 001's signature could never be
    satisfied and `migrate.sh apply` could not succeed on any database at all.
    """

    def test_the_uncast_sql_identifier_literal_still_yields_column_names(self):
        self.assertEqual(migrations._pk_columns("{org_id,account_id}"),
                         ("org_id", "account_id"))

    def test_a_properly_loaded_array_is_unchanged(self):
        self.assertEqual(migrations._pk_columns(["org_id", "account_id"]),
                         ("org_id", "account_id"))

    def test_a_single_column_key_is_not_split_into_characters(self):
        """The pre-migration shape. Read as characters it is neither the legacy key nor the
        composite one, so a database in a known state classified as an unknown one."""
        self.assertEqual(migrations._pk_columns("{account_id}"), ("account_id",))

    def test_an_empty_key_list_is_empty_not_a_pair_of_braces(self):
        self.assertEqual(migrations._pk_columns("{}"), ())

    def test_signatures_are_satisfiable_from_the_literal_shape(self):
        """The end-to-end consequence, in one assertion: a correct composite-key schema must
        classify as 001-applied even when the driver hands back the un-cast literal."""
        class Literal(FakeConnection):
            def execute(self, query, params=()):
                if "information_schema.table_constraints" in " ".join(query.split()):
                    return Result([("accounts", "{org_id,account_id}"),
                                   ("contacts", "{org_id,contact_id}"),
                                   ("activities", "{org_id,activity_id}")])
                return super().execute(query, params)

        self.assertTrue(migrations.schema_signatures(Literal())["001"])
        self.assertTrue(migrations.status(Literal())["ready"])


class StatefulConnection:
    """A fake that MUTATES, so `apply()` can actually be driven offline.

    `FakeConnection` above answers the four read queries and raises AssertionError on the
    `CREATE TABLE` / `INSERT` statements `apply()` issues, so the state machine that is the
    module's entire reason for existing — exactly-once tracking rows, the reconciled-vs-applied
    branch, the post-DDL signature assertion, the advisory lock, the per-migration transaction —
    could not be exercised by any test here. It was covered only by `migration-smoke.py`, which
    needs a live Postgres and is not in the stdlib-only gate. Reordering the INSERT before the
    signature check, or re-running 001's DDL against a reconciled database, left all 13 tests
    green.

    Models the two facts `status()` reads: which versions have a tracking ROW, and which have a
    schema SIGNATURE. Applying a DDL sets the signature; the INSERT adds the row.
    """

    def __init__(self, *, signatures=(), rows=None, tracking=False):
        self.signatures = set(signatures)
        self.rows = dict(rows or {})            # version -> checksum
        self.tracking = tracking or bool(self.rows)
        self.ddl_applied = []
        self.locks = []
        self.transactions = 0

    # `apply()` commits each DDL with its tracking row; a fake that ignored the boundary would
    # not notice a rollback, so entry/exit are counted and the tests assert on the count.
    def transaction(self):
        conn = self

        class _Tx:
            def __enter__(self_inner):
                conn.transactions += 1
                return self_inner

            def __exit__(self_inner, *exc):
                return False
        return _Tx()

    def execute(self, query, params=()):
        compact = " ".join(query.split())
        if "pg_advisory_lock" in compact or "pg_advisory_unlock" in compact:
            self.locks.append("lock" if "unlock" not in compact else "unlock")
            return Result([])
        if compact.upper().startswith("CREATE TABLE IF NOT EXISTS"):
            self.tracking = True
            return Result([])
        if compact.upper().startswith("INSERT INTO"):
            version, _name, checksum = params
            assert version not in self.rows, f"migration {version} recorded twice"
            self.rows[version] = checksum
            return Result([])
        if "to_regclass" in compact:
            return Result([("ironworks_schema_migrations",)] if self.tracking else [(None,)])
        if "information_schema.table_constraints" in compact:
            if "001" not in self.signatures:
                return Result([("accounts", ["account_id"])])
            return Result([("accounts", ["org_id", "account_id"]),
                           ("contacts", ["org_id", "contact_id"]),
                           ("activities", ["org_id", "activity_id"])])
        if "information_schema.columns" in compact:
            # Per table, for the reason the read-only fake next door gives: 004 is a column on
            # `activities`, and answering the accounts shape for both probes would let its
            # signature pass by reading a table it never touched.
            if "table_name = 'activities'" in compact:
                cols = ["activity_id", "org_id", "account_id", "body"]
                if "004" in self.signatures:
                    cols += ["contributor"]
            elif "table_name = 'accounts'" in compact:
                cols = ["account_id", "org_id"]
                if "002" in self.signatures:
                    cols += ["owner", "stage", "value_band"]
                if "003" in self.signatures:
                    cols += ["facts"]
            else:
                raise AssertionError(f"unexpected column probe: {compact}")
            return Result([(c,) for c in cols])
        if "FROM ironworks_schema_migrations" in compact:
            by_version = {x["version"]: x for x in migrations.expected()}
            return Result([(v, by_version[v]["name"], c, "2026-08-26T00:00:00+00:00")
                           for v, c in sorted(self.rows.items())])
        # Anything else is a migration DDL: mark the version it implements as present.
        for version, _name, path in migrations.MIGRATIONS:
            if migrations._ddl(path).strip() == query.strip():
                self.signatures.add(version)
                self.ddl_applied.append(version)
                return Result([])
        raise AssertionError(f"unexpected query: {compact}")


class Apply(unittest.TestCase):
    def test_a_fresh_database_applies_every_migration_exactly_once(self):
        conn = StatefulConnection()
        result = migrations.apply(conn)
        self.assertTrue(result["ready"], result)
        self.assertEqual([a["action"] for a in result["actions"]], ["applied"] * 4)
        self.assertEqual(conn.ddl_applied, ["001", "002", "003", "004"])
        self.assertEqual(sorted(conn.rows), ["001", "002", "003", "004"])
        self.assertEqual(conn.transactions, 4, "the DDL and its tracking row are one commit")
        self.assertEqual(conn.locks, ["lock", "unlock"])

    def test_a_second_run_changes_nothing(self):
        conn = StatefulConnection()
        migrations.apply(conn)
        before = dict(conn.rows)
        result = migrations.apply(conn)
        self.assertEqual([a["action"] for a in result["actions"]], ["already-applied"] * 4)
        self.assertEqual(conn.ddl_applied, ["001", "002", "003", "004"], "DDL was re-run")
        self.assertEqual(conn.rows, before)

    def test_a_schema_that_already_has_the_shape_is_RECONCILED_not_reapplied(self):
        """The branch a live database actually takes: the columns exist (an older hand-run
        migration) but nothing was recorded. Re-running the DDL there fails on Postgres —
        `constraint "accounts_pkey" of relation "accounts" does not exist` — so the distinction
        is the difference between converging and erroring out."""
        conn = StatefulConnection(signatures=("001", "002", "003", "004"))
        result = migrations.apply(conn)
        self.assertTrue(result["ready"], result)
        self.assertEqual([a["action"] for a in result["actions"]], ["reconciled"] * 4)
        self.assertEqual(conn.ddl_applied, [], "a reconciled schema had its DDL re-run")
        self.assertEqual(sorted(conn.rows), ["001", "002", "003", "004"])

    def test_a_migration_edited_after_application_is_refused(self):
        conn = StatefulConnection(signatures=("001", "002", "003", "004"),
                                  rows={"001": "0" * 64, "002": "0" * 64, "003": "0" * 64})
        with self.assertRaises(migrations.MigrationError) as caught:
            migrations.apply(conn)
        self.assertIn("checksum mismatch", str(caught.exception))
        self.assertEqual(conn.locks, ["lock", "unlock"], "the advisory lock was not released")

    def test_a_DDL_that_leaves_no_signature_is_refused_before_it_is_recorded(self):
        """The post-apply assertion, which is what keeps a half-applied migration from being
        recorded as done. Its ordering is the property: recording first would make the failure
        permanent and invisible."""
        conn = StatefulConnection()
        real_execute = conn.execute

        def ddl_is_a_no_op(query, params=()):
            """The DDL is accepted and the schema does not change — a migration file that
            silently did nothing, or one whose statements were rolled back."""
            for _version, _name, path in migrations.MIGRATIONS:
                if migrations._ddl(path).strip() == query.strip():
                    return Result([])
            return real_execute(query, params)

        conn.execute = ddl_is_a_no_op
        with self.assertRaises(migrations.MigrationError) as caught:
            migrations.apply(conn)
        self.assertIn("without its expected schema signature", str(caught.exception))
        self.assertEqual(conn.rows, {}, "a migration with no signature was still recorded")


if __name__ == "__main__":
    unittest.main(verbosity=2)
