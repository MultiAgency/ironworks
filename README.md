# Rust IronWorks

An operator-run seam that serves scoped, managed agentic services to organizations over a chat
channel, on top of official unmodified [IronClaw](https://github.com/nearai/ironclaw).

A tenant gets a service we operate on its behalf — not a runtime it configures, and not a toolkit
it builds with. Its records stay its own, its guidance shapes what the service says, and the
model never sees another tenant's anything.

**Status: it serves tenants over Telegram, live, survives crashing at every boundary either side
of a turn, reports what authority exists for an instance versus what it recorded creating, and
has answered one real question about real records.**
A real bot answered a real question through `crates/telegram` on 2026-08-31, and `relay` routes
each conversation to its own tenant from a registry that refuses as a whole rather than a row. On
2026-09-01 `coordinate` served the operator's own org from the live Account Service — two real
records, one answered turn, recovered byte-identically through a real upstream failure. That is
one turn, not a behavioural claim. Focused operator commands adopt existing authority, measure
confinement, publish a registry and export a complete scoped book. End-to-end provisioning and
deprovisioning are not built. The serving loop
runs continuously on one host for one tenant, which has answered and delivered, and
`dev/prove-host.sh` has now reached that host read-only for the first time — scored, with the one
failure attributed, in `DESIGN.md`'s status block, which is also where what is measured and what is
not built lives.

The first dogfooded product experience is **account coordination** for MultiAgency itself. Its
stable service id remains `relationship-intelligence`: it reads dated account records across
onboarding, delivery, renewal and offboarding, surfaces commitments and dependencies, and never
acts. It is the only service that ships, and there is no default: a tenant's registry entry names
a service or is refused.

## Where to start

**If you are an agent, start at `CLAUDE.md`.** Its "Which document answers which question" table is
the routing table — keyed on the question you have, which is the thing you actually know. What
follows here is an inventory keyed on the file, for a person arriving cold. Where the two would
disagree, the table in `CLAUDE.md` is the one to follow.

| File                           | What it holds |
| ------------------------------ | ------------- |
| `CLAUDE.md`                    | The always-loaded brief, the house rules, and the routing table. |
| `CONTEXT.md`                   | The glossary, and nothing else. Several terms are deliberately opinionated. |
| `DESIGN.md`                    | Types, layering, the state machine, one turn end to end, the crash boundaries, and what is measured. |
| `services/README.md`           | The service definitions, and why a configuration naming none is refused rather than defaulted. |
| `docs/PRD.md`                  | What each capability must become. A **target** document; a requirement in it is not evidence. |
| `docs/INVARIANTS.md`           | The bar, in lettered sections. `CONTEXT.md`'s Shorthand table says how to read one. |
| `docs/ACCOUNT-COORDINATION.md` | The dogfooded operator journey and its synthetic acceptance suite. |
| `docs/experiments/`            | Every measurement and its transcripts, indexed in its own `README.md`. |
| `docs/FRONTEND.md`             | What a rendered surface is for, and what it must never do. |
| `docs/UPGRADE.md`              | Every pin bump, what it was measured against, and the procedure for the next one. |
| `docs/adr/`                    | Decisions that are hard to reverse, and why. Indexed in its own `README.md`. |
| `docs/AUDIT-BRIEF.md`          | The prompt to hand an external auditor. |
| `deploy/README.md`             | The systemd units, the served profile, and the operator procedures. |

Everything else is where it looks: `scrap/` and `services/` hold what is compiled into the binary,
`fixtures/` synthetic records, `dev/` the probe stack and proof scripts, `site/` the generated
pages. `MODEL_PIN` and `IRONCLAW_PIN` are the model and runtime of record. **Before running
anything in `dev/`, read its `# COST:` line** — several spend money, one spends a runtime member
irreversibly, and one reaches a live host.

## The one rule

Run official, unmodified IronClaw. Everything here is configuration, data and code that runs
_around_ it. A change that requires editing IronClaw's source belongs upstream, not here.

It has two corollaries — one about pin defaults, one about what counts as a source for runtime
behaviour. They are stated once, in `CLAUDE.md` under the same heading, rather than in two files
that can drift apart. The upstream contract facts in `DESIGN.md` were read out of the pinned
revision, which is the second corollary applied.

## Relationship to the current IronWorks

`~/agency/ironworks-py` is the Python implementation this replaces — still deployed and still the
only system that provisions, but no longer developed (ADR 0011). This is a
**specification-compatible reimplementation, not a source-compatible port** — see
[ADR 0001](docs/adr/0001-reimplement-ironworks-in-rust.md). Trust guarantees, isolation,
lifecycle, delivery and acceptance behaviour must be preserved; Python structure, storage
schemas, file formats and module names need not be. **Preserved is the obligation, not the
claim** — lifecycle execution is partial and unqualified, and `docs/INVARIANTS.md` §F says what it
still owes.

That repo is therefore the **behavioural specification and adversarial test corpus**, not merely
a reference: nearly every method in its `multi/seam/bridge_state.py` and `bridge_core.py` carries
a docstring naming the defect it closed. Those are extracted in
[`docs/INVARIANTS.md`](docs/INVARIANTS.md), which is the actual bar for replacement.

## Fixtures

`fixtures/` is synthetic only — every record carries `_synthetic`, every domain is under `.example`
(RFC 2606), and real account data must never be added. `DESIGN.md`'s Proving Ground names the two
books and what each one traps.

## Licence

MIT or Apache-2.0, at your option.
