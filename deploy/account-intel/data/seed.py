#!/usr/bin/env python3
"""Deterministic TEST-MODE seed (dev-up.sh only; prod-up.sh seeds nothing).
Loads the fixture accounts + interaction notes into Postgres, idempotent (upsert
by id). Seeds two orgs to make org-isolation testable:
  org 'acme-sales'  — the four fixture accounts (Acme/Globex/Initech/Umbrella)
  org 'rival-sales' — a decoy 'Acme Corp' that must never leak into acme-sales scope
"""
import os, json, glob, pathlib
import psycopg

DSN = os.environ["ACCOUNT_DB_DSN"]
BASE = pathlib.Path(os.environ.get("FIXTURE_DIR", "/app/deploy/account-intel"))
ORG_A, ORG_B = "acme-sales", "rival-sales"

# account_id -> interaction files (activity evidence)
ACTIVITY_FILES = {
    "ACME-001":     [("ACME-INT-01", "2026-08-05", "call", "fixtures/interactions/acme-note.md")],
    "UMBRELLA-004": [("UMB-INT-01",  "2026-08-11", "call", "fixtures/interactions/umbrella-note.md")],
}

def upsert_account(cur, org, a):
    acc = a["account"]
    cur.execute("""INSERT INTO accounts
        (account_id,org_id,name,domain,industry,employees,headquarters,cloud,stated_problem,
         current_tooling,budget,timeline,decision_process,economic_buyer,
         owner,stage,value_band,facts,updated_at)
        VALUES (%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s, now())
        ON CONFLICT (org_id, account_id) DO UPDATE SET
         name=EXCLUDED.name,domain=EXCLUDED.domain,industry=EXCLUDED.industry,
         employees=EXCLUDED.employees,headquarters=EXCLUDED.headquarters,cloud=EXCLUDED.cloud,
         stated_problem=EXCLUDED.stated_problem,current_tooling=EXCLUDED.current_tooling,
         budget=EXCLUDED.budget,timeline=EXCLUDED.timeline,decision_process=EXCLUDED.decision_process,
         economic_buyer=EXCLUDED.economic_buyer,
         owner=EXCLUDED.owner,stage=EXCLUDED.stage,value_band=EXCLUDED.value_band,
         facts=EXCLUDED.facts,updated_at=now()""",
        (a["record_id"], org, acc["name"], acc.get("domain"), acc.get("industry"),
         acc.get("employees"), acc.get("headquarters"), json.dumps(acc.get("cloud")),
         acc.get("stated_problem"), acc.get("current_tooling"), acc.get("budget"),
         acc.get("timeline"), acc.get("decision_process"), acc.get("economic_buyer"),
         # Recorded team facts (migrate-002): the render path surfaces these and
         # test_ingress_fixes.py asserts they reach the model, but the seeder never
         # persisted them — so a book could not supply what the analyst is told to trust.
         acc.get("owner"), acc.get("stage"), acc.get("value_band"),
         # migrate-003: whatever keys THIS partner's book needs. The seam renders them in the
         # order the client's FACT_FIELDS declares and reports declared-but-absent as gaps,
         # so a book can be shaped per relationship instead of bent into B2B firmographics.
         json.dumps(acc["facts"]) if acc.get("facts") else None))
    for i, ct in enumerate(a.get("contacts", []), 1):
        cur.execute("""INSERT INTO contacts (contact_id,account_id,org_id,name,title,engaged,notes)
            VALUES (%s,%s,%s,%s,%s,%s,%s)
            ON CONFLICT (org_id, contact_id) DO UPDATE SET name=EXCLUDED.name,title=EXCLUDED.title,
             engaged=EXCLUDED.engaged,notes=EXCLUDED.notes""",
            (f'{a["record_id"]}-C{i}', a["record_id"], org, ct["name"], ct.get("title"),
             ct.get("engaged"), ct.get("notes")))
    # Activities arrive two ways — from a file fixture and inline on candidate fixtures — but
    # the row is the same row, so build one list and write it with a single statement instead
    # of two byte-identical INSERTs that can drift apart.
    activities = [(aid, a["record_id"], org, when, kind, (BASE / rel).read_text())
                  for (aid, when, kind, rel) in ACTIVITY_FILES.get(a["record_id"], [])]
    activities += [(act["activity_id"], a["record_id"], org, act.get("occurred_at"),
                    act.get("kind"), act["body"])
                   for act in a.get("activities", [])]
    if activities:
        cur.executemany("""INSERT INTO activities (activity_id,account_id,org_id,occurred_at,kind,body)
            VALUES (%s,%s,%s,%s,%s,%s)
            ON CONFLICT (org_id, activity_id) DO UPDATE SET body=EXCLUDED.body""", activities)

def main():
    with psycopg.connect(DSN) as conn, conn.cursor() as cur:
        for org, name in [(ORG_A, "Acme Sales Team"), (ORG_B, "Rival Sales Team")]:
            cur.execute("INSERT INTO organizations (org_id,name) VALUES (%s,%s) "
                        "ON CONFLICT (org_id) DO NOTHING", (org, name))
        for f in sorted(glob.glob(str(BASE / "fixtures/accounts/*.json"))):
            upsert_account(cur, ORG_A, json.load(open(f)))
        # decoy under org B — same display name, different id/data; must never leak to org A
        upsert_account(cur, ORG_B, {"record_id": "ACME-RIVAL-001",
            "account": {"name": "Acme Corp", "domain": "acme-rival.example",
                        "industry": "unrelated", "employees": 50}, "contacts": []})
        conn.commit()
        n = cur.execute("SELECT count(*) FROM accounts").fetchone()[0]
        print(f"seeded ok: {n} accounts across 2 orgs")

if __name__ == "__main__":
    main()
