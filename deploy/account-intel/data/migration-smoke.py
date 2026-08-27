#!/usr/bin/env python3
"""Destructive migration smoke test for an explicitly disposable Postgres database."""
from __future__ import annotations

import argparse
import json
import os
import urllib.parse

import psycopg
from psycopg.rows import dict_row

import migrations


BASELINE = """
CREATE TABLE organizations (
    org_id TEXT PRIMARY KEY, name TEXT NOT NULL, created_at TIMESTAMPTZ DEFAULT now()
);
CREATE TABLE accounts (
    account_id TEXT PRIMARY KEY, org_id TEXT NOT NULL REFERENCES organizations(org_id),
    name TEXT NOT NULL, domain TEXT, industry TEXT, employees INTEGER, headquarters TEXT,
    cloud JSONB, stated_problem TEXT, current_tooling TEXT, budget TEXT, timeline TEXT,
    decision_process TEXT, economic_buyer TEXT, updated_at TIMESTAMPTZ DEFAULT now()
);
CREATE INDEX accounts_org_idx ON accounts(org_id);
CREATE TABLE contacts (
    contact_id TEXT PRIMARY KEY, account_id TEXT REFERENCES accounts(account_id),
    org_id TEXT NOT NULL REFERENCES organizations(org_id), name TEXT NOT NULL,
    title TEXT, engaged BOOLEAN, notes TEXT
);
CREATE TABLE activities (
    activity_id TEXT PRIMARY KEY, account_id TEXT REFERENCES accounts(account_id),
    org_id TEXT NOT NULL REFERENCES organizations(org_id), occurred_at DATE,
    kind TEXT, body TEXT NOT NULL
);
INSERT INTO organizations(org_id, name) VALUES ('smoke-org', 'Smoke Org');
INSERT INTO accounts(account_id, org_id, name) VALUES ('account-1', 'smoke-org', 'Preserved');
"""


LOCAL_HOSTS = frozenset({"localhost", "127.0.0.1", "::1", ""})   # "" is a UNIX socket
DISPOSABLE_DBS = frozenset({"accounts_smoke", "smoke", "postgres_smoke"})


def _refuse_unless_disposable(dsn):
    """The guard on `DROP SCHEMA public CASCADE`, parsed rather than searched.

    It was `"localhost" not in dsn` — a SUBSTRING test, which passes for any DSN that merely
    contains the word (`?application_name=localhost`, a password holding it) and, far more
    likely, for the standard way an operator reaches a remote Postgres:

        postgresql://postgres:…@localhost:15432/accounts     # an SSH tunnel to production

    That is a local-looking DSN addressing the live client-data database, and the next
    statement drops its schema. It also REFUSED 127.0.0.1, which is as local as localhost.

    So: parse the host, and require a database name that production never carries. A tunnel
    forwards the port, not the database name, which is why the name is the half that holds."""
    if os.environ.get("MIGRATION_SMOKE_DISPOSABLE") != "1":
        raise SystemExit("refusing destructive setup: require MIGRATION_SMOKE_DISPOSABLE=1")
    parsed = urllib.parse.urlsplit(dsn)
    host = (parsed.hostname or "").strip("[]")
    database = parsed.path.lstrip("/")
    if host not in LOCAL_HOSTS:
        raise SystemExit(
            f"refusing destructive setup: DSN host is {host!r}, not one of "
            f"{sorted(LOCAL_HOSTS - {''})}")
    if database not in DISPOSABLE_DBS:
        raise SystemExit(
            f"refusing destructive setup: DSN database is {database!r}. A tunnel forwards the "
            f"PORT, not the name, so a localhost DSN can address production — name the "
            f"database one of {sorted(DISPOSABLE_DBS)} to prove it is disposable.")


def main(argv=None):
    parser = argparse.ArgumentParser()
    parser.add_argument("--prior", type=int, choices=range(4), required=True)
    args = parser.parse_args(argv)
    dsn = os.environ["ACCOUNT_DB_DSN"]
    _refuse_unless_disposable(dsn)

    # Match the Account Service pool exactly. A tuple-row smoke cannot detect readiness code
    # that accidentally indexes production ``dict_row`` mappings positionally.
    with psycopg.connect(dsn, autocommit=True, row_factory=dict_row) as conn:
        conn.execute("DROP SCHEMA public CASCADE")
        conn.execute("CREATE SCHEMA public")
        conn.execute(BASELINE)
        if args.prior:
            conn.execute(f"""
                CREATE TABLE {migrations.TRACKING_TABLE} (
                    version TEXT PRIMARY KEY, name TEXT NOT NULL, checksum TEXT NOT NULL,
                    applied_at TIMESTAMPTZ NOT NULL DEFAULT now())
            """)
        expected = migrations.expected()
        for index in range(args.prior):
            version, name, path = migrations.MIGRATIONS[index]
            conn.execute(migrations._ddl(path))
            conn.execute(
                f"INSERT INTO {migrations.TRACKING_TABLE}(version, name, checksum) "
                "VALUES(%s,%s,%s)", (version, name, expected[index]["checksum"]))

        first = migrations.apply(conn)
        second = migrations.apply(conn)
        recorded = conn.execute(
            f"SELECT version, count(*) FROM {migrations.TRACKING_TABLE} "
            "GROUP BY version ORDER BY version").fetchall()
        preserved = conn.execute(
            "SELECT name FROM accounts WHERE org_id='smoke-org' AND account_id='account-1'"
        ).fetchone()

    assert first["ready"] and second["ready"]
    assert all(action["action"] == "already-applied" for action in second["actions"])
    assert recorded == [{"version": "001", "count": 1},
                        {"version": "002", "count": 1},
                        {"version": "003", "count": 1}]
    assert preserved == {"name": "Preserved"}
    print(json.dumps({"prior": args.prior, "first": first["actions"],
                      "second": second["actions"], "data_preserved": True}, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
