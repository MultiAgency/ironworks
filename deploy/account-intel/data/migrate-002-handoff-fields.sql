-- Migration 002: pipeline-management fields for the handoff brief (matches schema.sql as of
-- 2026-08-20). Additive and nullable — no rewrite, no lock beyond the catalog update, and no
-- behaviour change for existing callers (the analyst's `missing` list is unaffected: these are
-- NOT in BUSINESS_FIELDS).
--
-- Why: a pipeline brief needs `owner` and `value_band`, and the store had no column for either,
-- so generating one would have forced the model to invent exactly the fields where the schema's
-- discipline matters most. These columns give both a real source; NULL now means "the team
-- hasn't recorded it", which a brief reports as UNKNOWN rather than filling in.
--
-- Apply once per live DB:
--   docker exec -i <account-db-container> psql -U postgres -d accounts < migrate-002-handoff-fields.sql
BEGIN;
ALTER TABLE accounts ADD COLUMN IF NOT EXISTS owner      TEXT;
ALTER TABLE accounts ADD COLUMN IF NOT EXISTS stage      TEXT;
ALTER TABLE accounts ADD COLUMN IF NOT EXISTS value_band TEXT;
COMMIT;

-- Verify:
--   \d accounts        -- owner, stage, value_band present and nullable
--   SELECT count(*) FROM accounts WHERE owner IS NOT NULL;   -- 0 until the team records any
