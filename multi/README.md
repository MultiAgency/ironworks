# multi — MultiAgency's main agent, on multi-tenant IronClaw

`multi` is the client-facing product: **one Multi agent serving many clients**, each sealed in
their own account, on a single multi-tenant IronClaw instance. Built on **unmodified**
`ironclaw:main` — config + persona + channels wrapped around the runtime, never a fork. That
boundary (no IronClaw source in here) is the security story.

A **component within `ironworks`** (the MultiAgency platform monorepo), not a separate repo:
`ironworks` already holds every piece of `multi`, and the pieces are entangled with the shared
tooling/personas, so they live together. `multi` graduates to its own repo only when it's a
deployed product with an independent lifecycle (own deploy, contributors, release cadence).

## Verified architecture (traced in the IronClaw source)

**Multi-tenant IronClaw gives DATA isolation, not per-account personas.**

- `agent` is a scope axis under tenant (`tenant/user/project/agent`) and cleanly seals
  data / memory / secrets per (tenant, user). Cross-account requests return `404`; no code
  execution (`SecureDefault`). *(proven behaviorally + in source)*
- There is **no shipped path to give each account its own agent/persona**: the v2 auth
  middleware stamps the host-configured `default_agent_id` onto every caller
  (`webui_serve.rs:855`), `admin/users` has no agent field, there is no `admin/agents`
  endpoint, and on the hosted-Postgres path the identity source degrades to **Empty**
  (`production_backend_assembly.rs:1614`) — the instance bakes no persona at all.
- Per-account personas would require **patching IronClaw core** (per-agent identity source +
  agent registration + per-user binding). **We do not patch core.**

**Therefore the persona lives in the CHANNEL/backend layer, not the instance.**

- Multi's persona and each account's business context are **injected per-turn** by the channel
  — exactly how the secretary bot already operates (persona via `instructions` every turn).
- The instance provides sealed-per-account data + memory + LLM plumbing.
- Per-client *customization*, if ever wanted, also lives here (channel injects per-account
  persona/context) — achievable **without touching IronClaw**.

> **Multi = a channel/backend-owned persona injected over a sealed-data multi-tenant IronClaw.**

## Isolation rules

- **No `IRONCLAW_REBORN_DEV_SECRET__*` on the multi-tenant instance — ever.** That surface is
  **tenant-wide, not per-account**: IronClaw resolves a keyed tool's credential caller-first, then
  falls back to a single tenant-shared admin-managed scope (verified: `ironclaw`
  `obligations/handler.rs` `secret_owner_scope`, caller-first-then-tenant-shared). One seeded
  `DEV_SECRET` therefore becomes usable by **every** sealed client's turns. Per-client credentials
  ride the seam (`ClientConfig`, `~/.agency/clients/`), never the instance env. The instance
  env allowlist in `instance/docker-compose.yml` is load-bearing: it carries no `DEV_SECRET__*`,
  and it must stay that way.
- **The production profile has no extension or skill lifecycle.** On `hosted_multi_tenant` /
  `production`, IronClaw's extension-lifecycle commands fail closed ("extension lifecycle is
  available only for standalone Reborn services") and there is **no multi-tenant profile
  upstream**. The seam owns Telegram, not an IronClaw extension, so this is coherent today — but
  any future "install an extension on the MT instance" plan is **blocked by profile** and would
  require patching core, which we do not do.

## Proven end-to-end

The full loop runs on the multi-tenant instance: on a fresh **sealed account**, the real
persona/skills injected via **`instructions`** + a client fixture injected as **`input`** → Multi
returns disciplined, evidence-tagged analysis that cites the client's actual facts, ranks the
genuinely-missing fields as UNKNOWNs, and stays read-only (7/7 context tells).

**The seam that makes it real (`the minimal context orchestrator`):**

    per turn, the channel/backend sends to the account's IronClaw scope:
      instructions = Multi's persona + skills          (baked NOWHERE on hosted-MT — must be injected)
      input        = ACCOUNT RECORDS envelope           (this account's data, from the Account Service)
                     + the user's request

**Built:** `multi/seam/context_ingress.py` injects the persona via
`instructions` every turn (`multi/seam/persona.py` composes it from the proven sources) AND routes
per-client credentials (`ClientConfig`, registry `~/.agency/clients/`). Proven live on the
multi-tenant instance: `verify/test_two_clients.py`, 11/11. (What remains is operational
infra, tracked in the operator's records outside this repo.)

## Layout

Gathered here. Operational status lives in the operator's records, outside
this repo.

    clients/       client-registry SCHEMA (the data lives in ~/.agency/clients, never in-repo)
    instance/      the multi-tenant IronClaw definition (secrets stay out of the repo; see .env.example)
    provision/     provision.sh — composed client provisioning (org + sealed account + registry);
                   provision-client.sh — the sealed-account mint primitive
    seam/          the product core: context_ingress.py (the trusted broker), persona.py,
                   telegram_bridge.py + unit suites + the runbook README
    serve/         the deployment units for the serve host: bridge + backup + watchdog
                   systemd units, their scripts, and cloud-init (SETUP.md, which names a
                   live host, is kept out of the repo)
    verify/        the reproducible proofs behind the verified architecture (+ their README)

Still in `ironworks` (not gathered — live / entangled with the product):

    agent/identity/ACCOUNT_INTELLIGENCE.md + skills/  Multi's proven persona + skills (channel-injected)
    deploy/secretary/worker/                          the LIVE front-desk secretary (Cloudflare Worker)
    deploy/account-intel/data/                        the Account Service (client-org data layer, running :8443)
