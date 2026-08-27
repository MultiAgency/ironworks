# IronWorks

IronWorks is MultiAgency's operator-run application layer around official, unmodified
[IronClaw](https://github.com/nearai/ironclaw). It supplies organization scope, service
composition, trusted business records, tenant lifecycle, answer-quality evaluation for the
account-analysis composition, and operational security.

Two services currently share one multi-tenant path:

- `account-analysis@1` — external organization-scoped account analysis;
- `relationship-intelligence@1` — MultiAgency's private relationship record, running on the
  canonical multi-tenant product path.

The product derives current commitments, obligations, relationship state, risk, and next actions
from durable account, contact, and activity records. Guidance carries policy; records carry facts.
It is not a sales pipeline, workflow engine, general ontology, self-service platform, or IronClaw
fork. New domain entities require a current use the existing record cannot represent.

## Architecture

```text
Telegram group
  -> one trusted bridge and private tenant registry
  -> organization-scoped Account Service context
  -> service definition + mandatory tenant guidance + safety tail
  -> tenant's sealed IronClaw account
  -> SQLite delivery journal and Telegram reply
```

The runtime owns model execution, threads, memory, tools, and sealed accounts; IronWorks owns the
adapter around it. An organization scopes business data and the tenant is the application boundary
that binds that scope to routing, a sealed member, guidance, and a service definition. Member
tokens belong exclusively to the seam, and runtime state is measured with operator commands rather
than documented as an inventory. [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) owns the
boundaries and what the adapter may and may not do; [`SECURITY.md`](SECURITY.md) owns the security
contract.

## Components

- `multi/services/`: committed service compositions.
- `multi/clients/`: registry and tenant-guidance schema; live files are private.
- `multi/seam/`: trusted context adapter, composition, bridge, redaction, and delivery state.
- `multi/provision/`: resumable tenant activation, confinement, and deletion.
- `multi/serve/`: private-host units, watchdog, and backups.
- `deploy/account-intel/`: organization-scoped Account Service.
- `multi/verify/`: runnable isolation and recovery proofs.
- `agent/identity/` and `skills/`: executable prompt data, not ordinary documentation.

## What else is in this repository

Two things here are operated and maintained but are **not** the canonical product path above.
Nothing in [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) or [`SECURITY.md`](SECURITY.md)
describes them, and they hold none of the tenant, organization, guidance, or service-definition
constructs that define IronWorks.

- **The single-tenant fleet** (`deploy/provision-agent.sh`, `deploy/doctor.sh`,
  `deploy/migrate-image.sh`, `deploy/update-persona.sh`, `deploy/enable-device-link.sh`,
  `deploy/vidgen/`, and the group personas in `agent/identity/`): one IronClaw instance, volume,
  bot, and baked persona per agent. Supported adjunct and shared operational tooling — it runs
  MultiAgency's own contributor agents and standalone group agents, shares `IRONCLAW_PIN`,
  `MODEL_PIN`, and `deploy/lib/`, and is covered by the same upgrade runbook. It is not an
  organization-scoped service and does not carry the product path's boundaries. Procedure:
  [`deploy/README.md`](deploy/README.md) § Fleet-agent handoff.
- **The Secretary** (`deploy/secretary/`): MultiAgency's own public front desk — a separate
  application built on IronClaw, running its own instance, volume, and trust domain behind a
  Cloudflare Worker. It shares this repository's pin, image, and secret conventions and nothing
  else. Details: [`deploy/secretary/worker/README.md`](deploy/secretary/worker/README.md).

Both run the pinned runtime, so both are in scope for [`deploy/UPGRADE.md`](deploy/UPGRADE.md).
Neither is in scope for the guarantees the product path documents.

## Quickstart and operator commands

Build `ironclaw:main` from `IRONCLAW_PIN`, prepare private values from
`multi/instance/.env.example`, and start the compose stack. Then:

```sh
multi/provision/provision.sh <slug> "<display name>" <group-id> --service <service> --dry-run
multi/provision/provision.sh <slug> "<display name>" <group-id> --service <service>

./deploy/ironworks doctor [--offline] [--json]
./deploy/ironworks tenants status
./deploy/ironworks tenant inspect <slug>
./deploy/ironworks bridge status
./deploy/ironworks egress status
./deploy/ironworks account-db migration-status
./deploy/ironworks service validate
./deploy/ironworks release verify [--offline-only]
./deploy/ironworks test
```

Private configuration, guidance, credentials, and runtime state live under `~/.agency/` or in
service volumes. A blocked or unevaluated check is not a pass.

Documentation map:

- [`docs/PRODUCT_DIRECTION.md`](docs/PRODUCT_DIRECTION.md): intent, maturity, and non-goals;
- [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md): topology, boundaries, and ownership;
- [`SECURITY.md`](SECURITY.md): security contract and known limitations;
- [`docs/EGRESS_CONTAINMENT.md`](docs/EGRESS_CONTAINMENT.md): network guarantee and operations;
- [`docs/BRIDGE_DELIVERY.md`](docs/BRIDGE_DELIVERY.md): delivery and recovery semantics;
- [`docs/INCIDENT_RESPONSE.md`](docs/INCIDENT_RESPONSE.md): procedures for failures with no
  in-product remedy;
- [`docs/IRONCLAW_RUNTIME_CONSTRAINTS.md`](docs/IRONCLAW_RUNTIME_CONSTRAINTS.md): pinned-runtime
  constraints and their probes;
- [ADR 0001: Reborn bridge compatibility boundary](docs/adr/0001-reborn-bridge-compatibility-boundary.md):
  accepted ownership decision and bridge-deletion gates;
- [ADR 0001 spike plan](docs/adr/0001-spike-plan.md): sequenced upstream
  experiments for satisfying those gates;
- [Shared-conversation admission constraint](docs/upstream-proposals/shared-conversation-admission.md):
  internal S1 compatibility-constraint record; no upstream action is planned;
- [Managed shared-conversation authority constraint](docs/upstream-proposals/organizational-conversation-authority.md):
  internal S2 compatibility-constraint record; the bridge remains required;
- [`multi/services/README.md`](multi/services/README.md): what a service definition is and what
  it does not yet do;
- [`deploy/README.md`](deploy/README.md): lifecycle, incidents, upgrades, and recovery;
- [`deploy/UPGRADE.md`](deploy/UPGRADE.md): the runtime pin-bump procedure of record;
- [`multi/verify/README.md`](multi/verify/README.md): runnable proof index.

## Repository rules

Current code, service JSON, tests, pins, and live commands outrank prose. `MODEL_PIN` is part of
the product promise and changes only through the verified upgrade procedure. Secrets and client
identities never enter the repository.

See [`CONTRIBUTING.md`](CONTRIBUTING.md). IronWorks is dual licensed under Apache-2.0 or MIT at
your option.
