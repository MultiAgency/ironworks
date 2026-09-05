---
status: accepted
date: 2026-09-04
refines: 0008
---

# Align one tenant's serving cell with one channel stream

## Context

ADR 0008 chose one seam process, bot, IronClaw member and journal per tenant while keeping the code
multi-tenant. This record makes that deployment boundary mechanical. It does not supersede ADR
0008.

The boundary cannot be drawn at the journal alone. `crates/telegram/src/main.rs::main` reads one
`TELEGRAM_BOT_TOKEN`, and that bot exposes one ordered update stream with one offset. The adapter
passes the offset supplied by the relay to that stream; it does not own another cursor.
`crates/channel/src/lib.rs::ChannelJournal::safe_cursor` therefore derives one safe offset as the
lowest open update in the journal, and ADR 0006 records why there must be only one answer for that
stream. Today one process, one journal and one stream span every registry entry. A tenant whose turn
stays open consequently holds the stream behind the same barrier for every other tenant.

The product requirement is stronger than eventual fairness: one tenant's failure must not change
another tenant's latency, delivery, consumable quota, or whether a turn runs. The unit of separation
must therefore contain every serving resource whose failure or budget one tenant can consume. A
common host or provider outage may still affect several cells; that is a platform failure, not one
tenant consuming another tenant's serving path.

## Decision

**The deployed serving unit is a channel-credential-aligned per-tenant serving cell.** Each admitted
client tenant gets:

- one channel credential and the one inbound stream and adapter process it names;
- one registry containing exactly that tenant, while retaining the ordinary multi-tenant schema and
  resolution path;
- one journal containing both turn and channel state for that stream;
- one relay supervisor loop, whose every cycle invokes a fresh `ironworks relay` process against
  that registry, journal and adapter; and
- one IronClaw member/runtime boundary and tenant-scoped quota-bearing credentials, as ADR 0008 and
  PRD §13 already require.

The journal boundary is the stream boundary. A tenant reachable through multiple rooms on one bot
still has one journal and one cursor because the provider stream has one cursor. This decision does
not settle PRD §15's future question about a tenant reachable through two independent channel
streams. Whatever settles it cannot give one provider stream multiple authorities for delivery
progress.

This is a deployment choice, not a single-tenant code path:

- §B-2 remains literal. Durable turn, thread and delivery identities continue to include
  `(org_id, <id>)`; a one-entry registry does not permit removing tenancy from a key or adding an
  ambient tenant.
- `crates/channel/src/lib.rs::ChannelJournal::acknowledge` remains unchanged in meaning. For the one
  stream represented by its journal, it recomputes the lowest-open barrier at the write and refuses
  both an offset beyond that barrier and an offset below the acknowledged value. Per-tenant does not
  mean one cursor per room or caller.
- `ironworks relay` remains one poll cycle and takes no daemon mode. A tenant's long-running unit
  supervises `dev/relay-loop.sh`; each cycle is a new relay process. B1–B12 therefore remain real
  process-death boundaries, and the next process recovers exclusively from that cell's durable
  journal.

Any quota that one tenant can exhaust must terminate inside the cell. The runtime request budget is
already per member. A channel or model provider credential may be shared only if its quota cannot be
consumed by one tenant to delay or refuse another; otherwise the credential is tenant-specific.

## Rejected alternatives

**One poller with per-tenant cursors** cannot represent the protocol. The bot has one stream and one
acknowledgement offset. Advancing it from a fast tenant's cursor can discard a slower tenant's open
update; taking the minimum preserves the present cross-tenant outage. Adding an aggregate cursor
beside the per-tenant cursors merely restores the shared barrier and creates two durable answers that
can disagree. It either violates ADR 0006 and §E-17 or fails the availability requirement.

**Per-tenant channel credentials without per-tenant serving cells** split the provider cursor but
not the failure domain. A shared relay process, journal, retry/backoff loop, runtime member or
consumable quota can still let one tenant delay delivery, crash the worker, exhaust the budget or
prevent another tenant's turn. Distinct credentials are necessary for Telegram, but alone are not
the mechanism; they are the outside edge of the chosen cell.

A journal and relay process per tenant while retaining one shared bot is likewise incomplete:
multiple processes cannot independently acknowledge one provider stream. That is why the selected
mechanism includes the channel credential rather than treating the first and third options as
competing choices.

## Availability consequence

This decision **closes only the cross-tenant half of the single-poller availability cap; it does not
accept that coupling as risk**. Once the cells are deployed, one token permits one poller for one
tenant, not one poller for every tenant, so a stopped poller cannot become every tenant's outage.
Until the units, credentials and journal paths are actually split and the isolation property is
measured with a positive control, that shared-poller cap remains and PRD §23 step 5 continues to
gate a second admitted client tenant.

The other half survives the split: each tenant still has one poller with no failover, and that
poller remains a single point of failure for its tenant. The architecture therefore removes one
tenant's poller failure from every other tenant's serving path, but it does not remove the bound on
a per-tenant service-level promise.
