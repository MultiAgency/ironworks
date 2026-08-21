-- Migration 003: per-partner flexible facts (matches schema.sql as of 2026-08-20).
-- Additive and nullable — no rewrite, no behaviour change for existing callers.
--
-- Why: the fixed columns encode ONE theory of what matters (a B2B sales account:
-- industry/employees/headquarters/economic_buyer/decision_process). Every partner's book is
-- shaped differently — one partner's rows are funded lines, another's may be
-- grantees, programmes, or venues — so for most books half the columns are meaningless and,
-- worse, the hardcoded `missing` list reported them as genuine gaps every turn. That trains
-- the reader to skim the one line where the value lives.
--
-- `facts` holds whatever keys THAT partner's relationship actually needs. Which keys matter is
-- declared per client in the registry (FACT_FIELDS) and explained in that client's guidance;
-- the seam renders what is present and reports the declared keys that are absent. The fixed
-- columns stay for books where they genuinely fit and are simply left NULL elsewhere.
--
-- Apply once per live DB:
--   docker exec -i <account-db-container> psql -U postgres -d accounts < migrate-003-facts.sql
BEGIN;
ALTER TABLE accounts ADD COLUMN IF NOT EXISTS facts JSONB;
COMMIT;

-- Verify:
--   \d accounts                                          -- facts present, nullable, jsonb
--   SELECT count(*) FROM accounts WHERE facts IS NOT NULL;   -- 0 until a row records any
