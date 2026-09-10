---
status: adopted
date: 2026-09-01
note: scope narrowed by the deployment review in this file
---

# Deploy one seam per tenant; keep the code multi-tenant

**Status: adopted for the sandbox-backed deployment, 2026-09-01; scope narrowed by the deployment
review below.** The code remains multi-tenant; the served IronClaw control plane and its configured
workspace root are provisioned once per client.

## Context

The seam is built multi-tenant: a registry, `route::resolve`, per-tenant credential handles,
`RecordScope`, a cursor that is the lowest open update across every tenant. The question is
whether that should also be how it is *deployed*.

Two facts settled it, and one of them reversed an earlier assumption.

**Runtimes are cheap.** Measured on the live box: an IronClaw instance idles at 25–130 MiB, with
the account service at 68 MiB. `mem_limit: 4g` is a ceiling, not a cost. Dozens of per-tenant
runtimes fit on the 15 GiB host already in use. The argument for sharing one — that per-tenant
runtimes would not fit — is simply false here.

**The cross-tenant barrier exists because of a shared bot.** Telegram has one global offset per
bot, so a fast tenant could acknowledge a slow tenant's unfinished work. That is why the safe
cursor spans tenants. One bot per tenant dissolves the problem rather than defending against it.

## Decision

**One seam process per tenant, with its own bot, its own IronClaw member, its own journal.**
This separates seam state, credentials, journals and configured storage roots. The decision does not
by itself establish a Docker-daemon trust boundary between control planes; the deployment contract
below now encodes one, and its live claim remains BLOCKED. Provisioning a synthetic two-instance
test deployment is how that claim is measured, not something the claim prevents.

**And keep the multi-tenant code.** A registry of one and a claim table of one are the same code
path — `relay` has no single-tenant mode, deliberately, so a channel cannot acquire a second and
looser route into the serving code. None of it is a name looking for a job: "the safe cursor is
the lowest open update" is simply the correct cursor rule, which happens to generalise.

## Consequences

- The intended strongest claim is structural rather than argued: *your records are handled by a
  process that has never held anyone else's credential.* The deployment contract now extends that
  structure to worker authority: each control plane has a client-owned rootless Docker daemon and
  no shared `docker` group (`deploy/user/ironworks-docker.service`;
  `deploy/ironworks-ironclaw@.service`). This remains a design claim until the two-client live proof
  passes on the deployment host.
- The middle option — shared runtime, member per tenant — is rejected because it **shares the
  expensive-to-secure thing to save the cheap thing.** The runtime is where the model executes.
- It resolves a live contradiction: the registry gives every tenant its own credential handles
  while `IRONWORKS_BASE` is read once from the environment. One runtime URL for every tenant in
  the process assumes a shared IronClaw; the fleet runs an instance per agent. Under this decision
  the global base URL is correct rather than an oversight.
- N systemd units instead of one. At tens of tenants, templated; at hundreds, wrong — that is
  IronClaw's `HostedMultiTenant` territory (Postgres-backed per-tenant filesystems, no process
  execution, per-user cost caps), where the runtime does the isolation and the seam follows. A
  rebuild to do when the scale exists, not a shape to adopt in advance of it.
- Each client is configured with a distinct IronClaw identity, home, workspace root and rootless
  daemon (`deploy/ironclaw-client.config.toml.example`; `deploy/ironworks-ironclaw@.service`).
  Threads for that client are intended to converge on the same `/workspace`. The preflight rejects
  default/mismatched identity and shared Docker-group membership before IronClaw starts.

## Narrowing: the repository encodes both boundaries; live proof remains

PRD §13 records two independent boundaries that narrow the original “isolation by deployment at
every layer” reading:

- The per-client deployment contract now requires its own rootless Docker daemon and socket, run by
  the same dedicated OS identity's user manager (`deploy/user/ironworks-docker.service`). The
  root-owned client environment names that account's runtime socket, the IronClaw unit grants no shared
  `docker` group, and startup rejects a mismatched endpoint (`deploy/ironworks-ironclaw@.service`;
  `deploy/check-ironclaw-instance.sh`). Distinct paths alone would not narrow daemon authority; the
  separate unprivileged daemon does.
- IronClaw derives its workspace key and container, proxy and network names from `(tenant_id,
  user_id)` alone
  (`ironclaw@4cb47cfaf3/crates/lanes/ironclaw_sandbox/src/sandbox_process/user_key.rs:24-59`). Both
  values default to `reborn-cli`
  (`ironclaw@4cb47cfaf3/crates/app/ironclaw_cli/src/commands/serve.rs:142-151`;
  `ironclaw@4cb47cfaf3/crates/app/ironclaw_cli/src/runtime/mod.rs:1322-1327`). The client config makes
  both explicit, and the preflight requires tenant, default owner, authenticated user and systemd
  instance to agree (`deploy/ironclaw-client.config.toml.example`;
  `deploy/check-ironclaw-instance.sh`).

Renaming cannot narrow shared daemon authority, and daemon separation does not excuse default
identity. `dev/prove-deployment-isolation.sh` therefore checks both, with same-name sentinels as
positive controls and cross-socket attempts performed as each service identity. That script has not
run on a deployment host. Under §F-14 the repository implementation is present and the deployment
claim remains BLOCKED, not PASS, until it does. The block applies to admitting a second client, not
to creating the synthetic pair the proof requires.

## The model-to-world boundary

Tenancy is isolation *between* customers. The separate problem is isolation between **one
tenant's model and the world**. The served topology therefore uses IronClaw 1.4.0's
`hosted-single-tenant-volume-sandboxed` profile: process execution occurs in the per-user worker,
the system filesystem is read-only, `/workspace` is the durable writable mount, and network access
goes through IronClaw's managed proxy. Tool policy still has to remove every capability the
service does not need; a sandbox contains execution but does not justify ambient tools.
