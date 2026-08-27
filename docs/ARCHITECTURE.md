# Architecture

IronWorks is the trusted application layer around official, unmodified IronClaw. This document
owns the current topology, trust boundaries, composition path, and component ownership. Runtime
state and deployment inventory come from operator commands, not this file.

This document describes the multi-tenant product path only. The single-tenant fleet and the
Secretary are listed in [`README.md`](../README.md) § What else is in this repository; they run
the pinned runtime but hold none of the constructs below, and nothing here describes them.

## Topology

```text
Telegram group
  -> trusted bridge: authenticate group and resolve tenant
  -> Account Service: fetch organization-scoped records
  -> seam: compose service persona + mandatory tenant guidance + safety tail
  -> tenant's sealed IronClaw account: run the model turn
  -> SQLite delivery journal
  -> Telegram reply
```

The bridge and Account Service run outside the model's authority. The seam holds both of a
tenant's credentials and uses each on exactly one leg: the Account Service is presented that
tenant's organization credential and nothing else, and IronClaw is authenticated with that
tenant's own sealed member bearer in the request's `Authorization` header — which is how the
runtime enforces the sealed-account boundary. No tenant's bearer is ever presented on another
tenant's turn, and the Account Service credential and host never leave the seam.

The IronClaw request body carries the selected model, composed instructions and tenant guidance,
user input, scoped business context, and an optional previous-response identifier. Neither
credential nor the Account Service address is included in that model-visible body. Instructions,
input, prior conversation, and model output may persist in IronClaw-managed response/thread state;
the IronWorks bridge stores only routing, response, context-version, and delivery identifiers.
This repository does not prove that IronClaw, a reverse proxy, or surrounding infrastructure
never records transport authentication headers in operational logs.

## Ownership

**IronClaw owns:** model execution, the agent loop, typed thread/turn/run state, model results,
memory, runtime tools, generic channel transport and delivery, runtime HTTP, and runtime/extension
credentials.

**IronWorks owns:** organization identity, tenant registry, service definitions, guidance
binding, authoritative scoped context, organizational routing policy, provisioning, confinement,
evaluation, application lifecycle, and operator controls. The current bridge's polling,
concurrency, thread pointer, and delivery journal are temporary compatibility mechanisms, not the
target ownership boundary. [`ADR 0001`](adr/0001-reborn-bridge-compatibility-boundary.md) records
why they cannot yet be removed and the six gates that permit deletion.

## Current migration status

IronWorks remains on the compatibility bridge because current stock Reborn cannot yet express the
required organization-scoped shared-conversation semantics safely. [`ADR 0001`](adr/0001-reborn-bridge-compatibility-boundary.md)
owns the deletion gates and accepted boundary; the [compatibility assessment](adr/0001-spike-plan.md)
owns their status and dated evidence.

Until that plan records all six gates passing against one official, unmodified Reborn revision,
do not begin bridge deletion. The bridge remains a required compatibility and security boundary
and is frozen to maintenance/correctness work only. Do not add generic runtime, channel, state,
concurrency, polling, response handling, or delivery responsibilities to it.

S1 and S2 are documented external constraints, not a contribution agenda:

- [production-bindable shared-conversation admission](upstream-proposals/shared-conversation-admission.md);
- [managed shared-conversation authority](upstream-proposals/organizational-conversation-authority.md).

Both are `UPSTREAM GAP — NO ACTION PLANNED`; `BRIDGE REMAINS REQUIRED`. Do not prepare or submit
upstream issues, contact maintainers, fork IronClaw, or plan implementation against hypothetical
upstream changes. Revisit the records only after a future official IronClaw release materially
changes shared-conversation admission or authority semantics.

Active IronWorks development remains focused on what IronWorks owns: organization and tenant
policy, Account Service authority and scoped context, service/guidance composition, provisioning
and deprovisioning, operator proofs and lifecycle, containment, and business-facing product
behavior.

The seam is an adapter and trust boundary. It may authenticate, resolve identity,
deterministically select bounded records, compose instructions, invoke IronClaw, and deliver the
result. It may not plan, score, choose actions, persist model-authored facts, or duplicate the
runtime's agent loop.

## Isolation and composition

An organization scopes business data; the **tenant** is the application boundary that isolation is
a property of. An organization owns the records, and a tenant binds that scope to everything else
that has to agree with it. Each tenant has:

- one registered group mapping;
- one organization-scoped Account Service identity;
- one sealed IronClaw member;
- one committed service definition;
- mandatory slug- and service-bound guidance held outside the repository.

Registry loading fails closed on unknown services, duplicate groups, missing credentials or
guidance, and slug/service-marker mismatches. Instructions are composed on every turn in the
order declared by the service definition. Registry changes require a bridge restart.

A tenant's organization is whichever one the Account Service authenticates from its credential;
registry `ORG_ID` is metadata and is not authoritative. Each persisted conversation is bound to
the composition it was built under — service, version, composed instructions, model, `FACT_FIELDS`
policy, authenticated organization, and Account Service endpoint — and a mismatch refuses
continuation until an operator resets that conversation explicitly. See
[`BRIDGE_DELIVERY.md`](BRIDGE_DELIVERY.md) for the identity and its one residual limitation, and
[`../deploy/README.md`](../deploy/README.md) for the operator procedure.

Member bearer tokens belong only to the seam. Per-bearer tool confinement and network egress
containment are independent controls; neither substitutes for the other. See
[`SECURITY.md`](../SECURITY.md) for the security contract.

## Components

- `multi/services/`: committed service compositions.
- `multi/clients/`: private-registry schema and guidance template.
- `multi/seam/`: registry loading, scoped context, composition, redaction, bridge, and SQLite
  delivery state.
- `multi/provision/`: resumable activation, confinement, and deletion.
- `multi/serve/`: private-host services, watchdog, and backups.
- `deploy/account-intel/`: organization-scoped Account Service.
- `multi/verify/`: runnable isolation, recovery, and behavior proofs.
- `agent/identity/` and `skills/`: executable prompt inputs selected by service definitions.

The serving path under `multi/seam/` must not import operator tooling from `deploy/`. Operator
tools may call product modules and proofs, but product serving must remain independently usable.

For subsystem interfaces, see [`multi/README.md`](../multi/README.md),
[`multi/services/README.md`](../multi/services/README.md), and
[`multi/clients/README.md`](../multi/clients/README.md).
