# account-intel — the data layer Multi's seam consumes

Account data and domain assets for the Multi product. The agent itself lives in
`multi/` (seam, provisioning, proofs); this directory supplies what it reads.

## What's here
- **`data/`** — the Account Service (multi-org, token-scoped, 404 cross-org) plus
  seeding and bring-up scripts (`dev-up.sh` for the laptop, `prod-up.sh` on the VM).
- **`fixtures/`** — demo accounts and interactions consumed by `data/seed.py` for
  dev bring-up and regression smoke.

## Schema lifecycle

`data/prod-up.sh` and `data/dev-up.sh` run `data/migrate.sh apply` before accepting readiness.
The runner records committed migration versions and checksums, reconciles an already-current
database without replaying DDL, and writes a private pre-migration dump under
`~/.agency/account-db-migrations/` only when work is required. `/health` is database liveness;
`/ready` additionally requires the committed schema and a loadable identity source.

Inspect without changing state:

```sh
./deploy/ironworks account-db migration-status
cd deploy/account-intel/data && ./migrate.sh status
```

Restore the named pre-migration dump to a replacement database for rollback; do not apply a
down-migration in place. The service does not report ready until the restored schema again matches
the committed migration set.

The service is reached by the **seam**, never by the model: a WASM extension calling it
directly is impossible on this runtime (the sandbox denies private-IP egress unconditionally).
That is the reason the seam exists, and it is written up with its source citations in
`multi/README.md` § Why the adapter exists, and why not a tool extension (do not reopen).

## Persona and skills (live, used by the product)
- `agent/identity/ANALYST.md` + `skills/account-analysis/SKILL.md` — `account-analysis@1`,
  the client-generic composition every external tenant runs.
- `agent/identity/RELATIONSHIP_INTELLIGENCE.md` + `skills/relationship-record/SKILL.md` —
  `relationship-intelligence@1`, MultiAgency's internal composition: derive relationship state
  from dated activities, read-only boundary.

Both carry the same evidence discipline (FACT/STATED/INFERENCE/HYPOTHESIS/UNKNOWN) and are
composed per turn by `multi/seam/persona.py` from the service definitions in
`multi/services/` (byte-identical in `multi/verify/test_product_loop.py`).

## Runtime facts
- `/workspace` is a per-caller, DB-backed mount — seed through the runtime, not
  `docker cp`/`docker exec`. Company knowledge is a skill (agent-global) for the
  same reason.
- IronClaw tool-disable is per-bearer with no tenant-global scope (upstream
  nearai/ironclaw#7310) — the enforcement pattern is `multi/provision/confine-member.sh`
  (allowlist, applied with the member's own token, probed fail-closed); what the
  per-bearer scope costs is stated in `docs/ARCHITECTURE.md` § Isolation and composition, and
  the network boundary underneath it is `docs/EGRESS_CONTAINMENT.md`.
