#!/usr/bin/env python3
"""Seed REAL account FACTS into a real org. Reads *.json (candidate shape: record_id,
account{}, contacts[], activities[]) from REAL_DATA_DIR and upserts into SALES_ORG.

Real customer data lives OUTSIDE the repo (~/.agency/account-data/<org>/); seed-real.sh
copies it into the account-service container and runs this there. Idempotent (reuses
seed.upsert_account). Facts only — never scores, gaps, or judgements (those stay ephemeral
and are recomputed by the agent).
"""
import os, json, glob, pathlib
import psycopg
from seed import upsert_account   # same dir; identical upsert (accounts + contacts + inline activities)

DSN = os.environ["ACCOUNT_DB_DSN"]
DATA = pathlib.Path(os.environ["REAL_DATA_DIR"])
ORG = os.environ["SALES_ORG"]
ORG_NAME = os.environ.get("SALES_ORG_NAME", ORG)


def main():
    files = sorted(glob.glob(str(DATA / "*.json")))
    if not files:
        raise SystemExit(f"no *.json in {DATA}")
    with psycopg.connect(DSN) as conn, conn.cursor() as cur:
        cur.execute("INSERT INTO organizations (org_id,name) VALUES (%s,%s) "
                    "ON CONFLICT (org_id) DO NOTHING", (ORG, ORG_NAME))
        for f in files:
            upsert_account(cur, ORG, json.load(open(f)))
        conn.commit()
        n = cur.execute("SELECT count(*) FROM accounts WHERE org_id=%s", (ORG,)).fetchone()[0]
        print(f"seeded {n} accounts into org '{ORG}' from {len(files)} files")


if __name__ == "__main__":
    main()
