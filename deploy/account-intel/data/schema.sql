-- MultiAgency internal Account Store — business FACTS only.
-- The agent never sees this schema; the Account Service maps rows -> the business contract,
-- so this DB can evolve independently. No qualification_score / discovery_gap / confidence.

CREATE TABLE IF NOT EXISTS organizations (
    org_id      TEXT PRIMARY KEY,            -- trusted scope key (config/identity, never model-supplied)
    name        TEXT NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- TENANCY IN THE KEYS, not just the WHERE clauses: every table is keyed by (org_id, <id>),
-- so an id collision between two orgs creates two independent rows — it can never re-home
-- or overwrite another org's data (migrate-001-org-scoped-keys.sql upgrades live DBs).
CREATE TABLE IF NOT EXISTS accounts (
    account_id      TEXT NOT NULL,            -- stable record id (e.g. ACME-001), unique PER ORG
    org_id          TEXT NOT NULL REFERENCES organizations(org_id),
    name            TEXT NOT NULL,
    domain          TEXT,
    industry        TEXT,
    employees       INTEGER,
    headquarters    TEXT,
    cloud           JSONB,                    -- {providers, footprint}
    stated_problem  TEXT,                     -- NULL = genuinely unknown (explicit)
    current_tooling TEXT,
    budget          TEXT,                     -- NULL = unknown; source truth, not a derived judgement
    timeline        TEXT,
    decision_process TEXT,
    economic_buyer  TEXT,
    -- Pipeline-management facts, recorded by the CLIENT'S TEAM (never derived by the model).
    -- They exist so a pipeline brief has a real source for owner/stage/value_band instead of
    -- inventing them; NULL means "the team hasn't recorded it", which a brief must report as
    -- UNKNOWN. Deliberately NOT in BUSINESS_FIELDS: those drive the analyst's qualification
    -- discipline, and these are pipeline bookkeeping, not qualification evidence.
    owner           TEXT,                     -- who on the client's team owns this account
    stage           TEXT,                     -- the client's own pipeline stage label
    value_band      TEXT,                     -- coarse recorded size band, never a model estimate
    -- Per-partner flexible facts (migration 003). The fixed columns above encode ONE theory of
    -- what matters (a B2B sales account); every book is shaped differently. `facts` holds the
    -- keys THAT relationship needs; the registry's FACT_FIELDS declares which of them are
    -- expected, and the seam reports the declared-but-absent ones as the gaps.
    facts           JSONB,
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (org_id, account_id)
);
CREATE INDEX IF NOT EXISTS accounts_name_idx ON accounts(org_id, lower(name));

CREATE TABLE IF NOT EXISTS contacts (
    contact_id  TEXT NOT NULL,
    account_id  TEXT NOT NULL,
    org_id      TEXT NOT NULL REFERENCES organizations(org_id),
    name        TEXT NOT NULL,
    title       TEXT,
    engaged     BOOLEAN,
    notes       TEXT,
    PRIMARY KEY (org_id, contact_id),
    FOREIGN KEY (org_id, account_id) REFERENCES accounts(org_id, account_id)
);
CREATE INDEX IF NOT EXISTS contacts_account_idx ON contacts(org_id, account_id);

CREATE TABLE IF NOT EXISTS activities (
    activity_id TEXT NOT NULL,                 -- stable id (e.g. ACME-INT-01), unique PER ORG
    account_id  TEXT NOT NULL,
    org_id      TEXT NOT NULL REFERENCES organizations(org_id),
    occurred_at DATE,
    kind        TEXT,                          -- call | note | email ...
    body        TEXT NOT NULL,                 -- the interaction text (evidence)
    PRIMARY KEY (org_id, activity_id),
    FOREIGN KEY (org_id, account_id) REFERENCES accounts(org_id, account_id)
);
CREATE INDEX IF NOT EXISTS activities_account_idx ON activities(org_id, account_id);
