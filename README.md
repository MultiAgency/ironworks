# ironworks — the MultiAgency agent fleet

This repo is the config + tooling for **MultiAgency**, all of it running on the
**official, unmodified [ironclaw](https://github.com/nearai/ironclaw) binary** (the
agent OS / harness). **No custom binary, no fork, no edits to ironclaw** — everything
here is config, data (personas), and scripts.

**The product is [`multi/`](multi/)**: one Multi agent serving many clients, each
sealed in their own account on a single **multi-tenant** ironclaw instance. The
instance seals the data; the persona and each client's business context are injected
**per turn** by the channel seam. Data isolation between clients is harness-enforced
and proven on both the compliant and the hostile path (cross-account reads 404, no
code-exec — see [`multi/verify/`](multi/verify/)).

Alongside it runs **the internal control plane** — **Multron**, the crew's own
coordination agent, an isolated single-tenant ironclaw instance (own container, bot,
persona, memory). Earlier control-plane agents were retired; `deploy/provision-agent.sh`
stands up a replacement from any persona file, so the pattern outlives any one of them.

This repo — **`ironworks`** — is the foundry that builds and runs all of it; the
system is **MultiAgency**.

## The one rule

> Run unmodified official ironclaw. Everything here is config, data, and tooling.

The old setup ran a *patched ironclaw fork*, so every upstream pull conflicted. Now
nothing here edits ironclaw's tree — pulling upstream is a clean rebuild. Pull updates =
bump the pinned rev, rebuild the image, re-provision.

The version of record for ironclaw is the one-line `IRONCLAW_PIN` file at this repo's
root — the rev every image is built from, bumped only via `deploy/UPGRADE.md`. Between a
pin edit and the rebuild that follows it the fleet still serves the *previous* rev; the
bump commit records both, and `deploy/UPGRADE.md` is the procedure that closes the gap.
"Verified at the pinned rev" elsewhere in the docs means a fact was checked against
whatever `IRONCLAW_PIN` held at the time — so a bump is what obliges re-checking it, not
the passage of time.

There is a second pin beside it: **`MODEL_PIN`** names the model of record for every
agent, and — unusually for a one-line file — says *why*, because the choice is a product
promise and not just a cost line. Changing it changes what can honestly be claimed about
where prompts are processed, so the file asks you to re-test behaviour before editing it.

`MODEL_PIN` has the same edit-to-deployment gap as `IRONCLAW_PIN` above, and it does not
close everywhere at once — so after a bump the fleet is briefly split across two models.
The proofs read the pin per run, so they move immediately. The seam reads it at import
(`multi/seam/context_ingress.py`), so the bridge moves on its next restart. The secretary
Worker bundles it at BUILD time, so it moves only on the next `wrangler deploy` — it is
the laggard, and it is the visitor-facing one. Bump the pin, then redeploy the Worker;
until you do, the front desk is still answering on the previous model and nothing in a
reply will say so.

## Quickstart

You bring an [official ironclaw](https://github.com/nearai/ironclaw) build and a few
credentials; the scripts here do the wiring.

**Prerequisites**
- Docker, and the `ironclaw:main` image built locally from the **unmodified** official source
- a [NEAR AI](https://near.ai) API key (the model gateway)
- a Telegram bot from BotFather (token + username) — the one manual step per agent
- a named `cloudflared` tunnel for inbound webhooks

**Run the multi-tenant client product** — one instance, many sealed client accounts:
copy [`multi/instance/.env.example`](multi/instance/.env.example) to `.env` and fill it,
bring up `multi/instance/`, then provision each client with
[`multi/provision/provision-client.sh`](multi/provision/provision-client.sh). Data
isolation between accounts is harness-enforced (proven: cross-account reads 404, no
code-exec). See [`multi/`](multi/).

**Stand up one isolated internal agent (the fleet model)**
1. Build the `ironclaw:main` image from the official source (unmodified).
2. Create the bot with BotFather; have your NEAR AI key ready.
3. Run `deploy/provision-agent.sh` — its header documents the exact arguments and env it expects.
4. Verify with `deploy/doctor.sh` (health-checks one agent or the whole fleet).

## What's here

- **`multi/`** — the product: the multi-tenant instance definition, the channel seam
  (persona + context injection, Telegram bridge), client provisioning, the registry
  schema, and the reproducible isolation proofs (`multi/verify/`).
- **`agent/identity/`** — the persona files (product, control-plane, template,
  frozen-experiment personas + the shared safety tail). The exhaustive inventory,
  and the only place their count is stated, is `docs/ARCHITECTURE.md` § Personas.
- **`skills/`** — skill files composed into the product personas per turn.
- **`deploy/`** — fleet tooling (`provision-agent.sh`, `doctor.sh`) and
  the supporting live services (`secretary/`, `account-intel/`); per-subdirectory
  status in `deploy/README.md`.
- **`docs/ARCHITECTURE.md`** — how it all fits (topology of record).

## Isolation (why it's safe)

Three layers, all harness-enforced:
- **Between agents** — each is a separate ironclaw instance with its own state store; one
  agent cannot see another's data.
- **Within an agent** — each member's turns run *as them* (run-acts-as-invoker), with their own memory.
  Unpaired actors fail closed.
- **Between clients on the product instance** — one sealed account per client:
  cross-account reads 404, no code-exec, no token crossover — proven on the compliant
  path, the hostile path, and the tenant-wide surfaces (`multi/verify/`).

## Runtime state & secrets

State (users, pairings, memory, threads) is **not** in this repo — it lives in each
instance's data volume. Per-agent secrets live outside the repo under `~/.agency/`
(operator tokens, webhook secrets, bot tokens) and are gitignored. This repo is the
*source of what the fleet is*; the volumes are where instances *run*.

## Historical note

An earlier direction explored native harness features (guest admission for anonymous
participants in shared groups, a per-channel capability floor, suspension enforcement).
The fleet uses **instance-per-agent + per-member pairing** instead, which needs none of
it. That work is frozen as the `guest-admission-reference` tag in the ironclaw repo — not
a dependency.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option — the same dual license as [ironclaw](https://github.com/nearai/ironclaw),
which this repo builds on. **No ironclaw source is vendored here at all** — not a file, not an
interface contract. Everything in this repo is config, data (personas), and scripts that run
around the stock binary.

Unless you explicitly state otherwise, any contribution intentionally submitted for
inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed
as above, without any additional terms or conditions.
