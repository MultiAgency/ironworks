-- Migration 001: tenancy into the keys (matches schema.sql as of 2026-08-19).
-- Global id PKs let a cross-org id collision re-home a tenant's rows (seed upsert set
-- org_id=EXCLUDED.org_id). Composite (org_id, <id>) keys make that structurally impossible.
-- Apply once per live DB:
--   docker exec -i <account-db-container> psql -U postgres -d accounts < migrate-001-org-scoped-keys.sql
BEGIN;
ALTER TABLE contacts   DROP CONSTRAINT IF EXISTS contacts_account_id_fkey;
ALTER TABLE activities DROP CONSTRAINT IF EXISTS activities_account_id_fkey;

ALTER TABLE accounts   DROP CONSTRAINT accounts_pkey;
ALTER TABLE accounts   ADD PRIMARY KEY (org_id, account_id);

-- REPAIR: heal rows already re-homed by the pre-migration hijack bug. If child rows exist
-- for an (org, account) pair with no account row (the account was stolen by a later org's
-- seed under the global PK), restore a copy of the account into the orphaned org.
INSERT INTO accounts (org_id, account_id, name, domain, industry, employees, headquarters,
                      cloud, stated_problem, current_tooling, budget, timeline,
                      decision_process, economic_buyer)
SELECT DISTINCT c.org_id, a.account_id, a.name, a.domain, a.industry, a.employees,
                a.headquarters, a.cloud, a.stated_problem, a.current_tooling, a.budget,
                a.timeline, a.decision_process, a.economic_buyer
FROM (SELECT org_id, account_id FROM contacts
      UNION SELECT org_id, account_id FROM activities) c
JOIN accounts a ON a.account_id = c.account_id
WHERE NOT EXISTS (SELECT 1 FROM accounts x
                  WHERE x.org_id = c.org_id AND x.account_id = c.account_id)
ON CONFLICT DO NOTHING;

ALTER TABLE contacts   DROP CONSTRAINT contacts_pkey;
ALTER TABLE contacts   ADD PRIMARY KEY (org_id, contact_id);
ALTER TABLE contacts   ADD FOREIGN KEY (org_id, account_id) REFERENCES accounts(org_id, account_id);

ALTER TABLE activities DROP CONSTRAINT activities_pkey;
ALTER TABLE activities ADD PRIMARY KEY (org_id, activity_id);
ALTER TABLE activities ADD FOREIGN KEY (org_id, account_id) REFERENCES accounts(org_id, account_id);

DROP INDEX IF EXISTS accounts_org_idx;   -- redundant: the new PK leads on org_id
COMMIT;
