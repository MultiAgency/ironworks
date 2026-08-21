# account-intel — the data layer Multi's seam consumes

Account data and domain assets for the Multi product. The agent itself lives in
`multi/` (seam, provisioning, proofs); this directory supplies what it reads.

## What's here
- **`data/`** — the Account Service (multi-org, token-scoped, 404 cross-org) plus
  seeding and bring-up scripts (`dev-up.sh` for the laptop, `prod-up.sh` on the VM).
- **`fixtures/`** — demo accounts and interactions consumed by `data/seed.py` for
  dev bring-up and regression smoke.

The service is reached by the **seam**, never by the model: a WASM extension calling it
directly is impossible on this runtime (the sandbox denies private-IP egress unconditionally).
That is the reason the seam exists, and it is written up with its source citations in
`multi/seam/README.md` § "Why it exists (don't reopen)".

## Persona and skills (live, used by the product)
- `agent/identity/ACCOUNT_INTELLIGENCE.md` — identity, evidence discipline
  (FACT/INFERENCE/HYPOTHESIS/UNKNOWN), read-only boundary.
- `skills/account-intelligence/SKILL.md` — the qualify→discover methodology.
- `skills/company-knowledge/SKILL.md` — ICP, qualification criteria, positioning.

These three files are the Multi persona, composed per turn by
`multi/seam/persona.py` (byte-identical in `multi/verify/test_product_loop.py`).

## Runtime facts
- `/workspace` is a per-caller, DB-backed mount — seed through the runtime, not
  `docker cp`/`docker exec`. Company knowledge is a skill (agent-global) for the
  same reason.
- IronClaw tool-disable is per-bearer with no tenant-global scope (upstream
  nearai/ironclaw#7310) — the enforcement pattern is `multi/provision/confine-member.sh`
  (allowlist, applied with the member's own token, probed fail-closed); what the
  per-bearer scope costs is stated in `docs/ARCHITECTURE.md` § Token custody.
