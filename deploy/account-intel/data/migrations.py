#!/usr/bin/env python3
"""Versioned Account Store migrations and schema-readiness checks.

The migration files remain the reviewable source of DDL.  This module supplies the missing
state machine around them: checksums, exactly-once records, legacy-schema reconciliation, and
post-apply validation.  It deliberately accepts a DB-API-like connection so the contract can be
unit-tested without importing psycopg or starting Postgres.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import pathlib
from collections.abc import Mapping


HERE = pathlib.Path(__file__).resolve().parent
MIGRATIONS = (
    ("001", "org-scoped-keys", HERE / "migrate-001-org-scoped-keys.sql"),
    ("002", "handoff-fields", HERE / "migrate-002-handoff-fields.sql"),
    ("003", "facts", HERE / "migrate-003-facts.sql"),
)
TRACKING_TABLE = "ironworks_schema_migrations"


class MigrationError(RuntimeError):
    """The database cannot be proved to match the committed migration contract."""


def _checksum(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def expected():
    return [{"version": version, "name": name, "checksum": _checksum(path)}
            for version, name, path in MIGRATIONS]


def _values(row):
    """Return database columns in query order for tuple rows and psycopg ``dict_row`` rows."""
    if isinstance(row, Mapping):
        return tuple(row.values())
    return tuple(row)


def _rows(conn, query, params=()):
    cur = conn.execute(query, params)
    rows = cur.fetchall()
    return [_values(r) for r in rows]


def _tracking_exists(conn):
    row = conn.execute("SELECT to_regclass('public.ironworks_schema_migrations')").fetchone()
    return bool(row and _values(row)[0])


def _pk_columns(value):
    """One primary key's columns, from either shape `array_agg` can arrive in.

    `information_schema.key_column_usage.column_name` is `sql_identifier`, not `text`, so an
    UN-CAST `array_agg` of it returns `sql_identifier[]` — an OID psycopg has no array loader
    for. The value then arrives as the LITERAL string '{org_id,account_id}', and `tuple()` on
    that walks it one character at a time. Measured, not theorised: that made
    `schema_signatures()["001"]` unsatisfiable on EVERY database, so `apply()` raised
    MigrationError immediately after a migration that had in fact succeeded, and `/ready`
    answered 503 permanently. The query below casts to text so this receives a list; this
    normalises both shapes so re-editing the query cannot quietly bring the defect back.
    """
    if isinstance(value, str):                      # the un-cast sql_identifier[] literal
        return tuple(c for c in value.strip("{}").split(",") if c)
    return tuple(value)


def _primary_keys(conn):
    rows = _rows(conn, """
        SELECT tc.table_name, array_agg(kcu.column_name::text ORDER BY kcu.ordinal_position)
          FROM information_schema.table_constraints tc
          JOIN information_schema.key_column_usage kcu
            ON tc.constraint_name = kcu.constraint_name
           AND tc.constraint_schema = kcu.constraint_schema
         WHERE tc.constraint_schema = 'public'
           AND tc.constraint_type = 'PRIMARY KEY'
           AND tc.table_name IN ('accounts', 'contacts', 'activities')
         GROUP BY tc.table_name
    """)
    return {table: _pk_columns(columns) for table, columns in rows}


def _account_columns(conn):
    return {row[0] for row in _rows(conn, """
        SELECT column_name FROM information_schema.columns
         WHERE table_schema = 'public' AND table_name = 'accounts'
    """)}


def schema_signatures(conn):
    pks = _primary_keys(conn)
    cols = _account_columns(conn)
    return {
        "001": all(pks.get(table) == ("org_id", ident) for table, ident in (
            ("accounts", "account_id"), ("contacts", "contact_id"),
            ("activities", "activity_id"))),
        "002": {"owner", "stage", "value_band"}.issubset(cols),
        "003": "facts" in cols,
    }


def status(conn):
    want = expected()
    signatures = schema_signatures(conn)
    applied = {}
    if _tracking_exists(conn):
        applied = {str(version): {"name": name, "checksum": checksum,
                                  "applied_at": str(applied_at)}
                   for version, name, checksum, applied_at in _rows(
                       conn, "SELECT version, name, checksum, applied_at "
                             "FROM ironworks_schema_migrations ORDER BY version")}
    problems = []
    for item in want:
        version = item["version"]
        recorded = applied.get(version)
        if not recorded:
            problems.append(f"migration {version} is not recorded")
        elif recorded["checksum"] != item["checksum"]:
            problems.append(f"migration {version} checksum differs from the committed file")
        if not signatures.get(version):
            problems.append(f"migration {version} schema signature is absent")
    unknown = sorted(set(applied) - {item["version"] for item in want})
    if unknown:
        problems.append("unknown applied migration(s): " + ", ".join(unknown))
    return {"ready": not problems, "expected": want, "applied": applied,
            "signatures": signatures, "problems": problems}


def _ddl(path):
    # Each file is independently transactional for manual psql use.  The runner owns the
    # transaction so it can commit the DDL and its tracking row atomically.
    lines = path.read_text().splitlines()
    return "\n".join(line for line in lines
                     if line.strip().upper() not in {"BEGIN;", "COMMIT;"})


def apply(conn):
    conn.execute("SELECT pg_advisory_lock(491927431)")
    actions = []
    try:
        conn.execute(f"""
            CREATE TABLE IF NOT EXISTS {TRACKING_TABLE} (
                version TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                checksum TEXT NOT NULL,
                applied_at TIMESTAMPTZ NOT NULL DEFAULT now()
            )
        """)
        for version, name, path in MIGRATIONS:
            checksum = _checksum(path)
            current = status(conn)
            recorded = current["applied"].get(version)
            if recorded:
                if recorded["checksum"] != checksum:
                    raise MigrationError(
                        f"migration {version} was changed after application; checksum mismatch")
                if not current["signatures"].get(version):
                    raise MigrationError(
                        f"migration {version} is recorded but its schema signature is absent")
                actions.append({"version": version, "action": "already-applied"})
                continue
            action = "reconciled" if current["signatures"].get(version) else "applied"
            with conn.transaction():
                if action == "applied":
                    conn.execute(_ddl(path))
                if not schema_signatures(conn).get(version):
                    raise MigrationError(
                        f"migration {version} completed without its expected schema signature")
                conn.execute(
                    f"INSERT INTO {TRACKING_TABLE}(version, name, checksum) VALUES(%s,%s,%s)",
                    (version, name, checksum))
            actions.append({"version": version, "action": action})
        final = status(conn)
        if not final["ready"]:
            raise MigrationError("schema is not ready after migration: "
                                 + "; ".join(final["problems"]))
        return {"ready": True, "actions": actions, "status": final}
    finally:
        conn.execute("SELECT pg_advisory_unlock(491927431)")


def main(argv=None):
    parser = argparse.ArgumentParser()
    parser.add_argument("action", choices=("status", "apply"))
    args = parser.parse_args(argv)
    import os
    import psycopg

    try:
        with psycopg.connect(os.environ["ACCOUNT_DB_DSN"], autocommit=True) as conn:
            result = status(conn) if args.action == "status" else apply(conn)
    except Exception as error:
        print(json.dumps({"ready": False, "error": type(error).__name__,
                          "detail": str(error)}))
        return 2
    print(json.dumps(result, indent=2, sort_keys=True))
    return 0 if result.get("ready") else 2


if __name__ == "__main__":
    raise SystemExit(main())
