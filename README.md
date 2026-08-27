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
one turn, not a behavioural claim. It does not provision or deprovision, and nothing runs the
serving loop anywhere yet — `dev/relay-loop.sh` exists and is undeployed. See the status table in
`DESIGN.md` for what is measured and what is not built.

The first dogfooded product experience is **account coordination** for MultiAgency itself. Its
stable service id remains `relationship-intelligence`: it reads dated account records across
onboarding, delivery, renewal and offboarding, surfaces commitments and dependencies, and never
acts. It is the only service that ships, and there is no default: a tenant's registry entry names
a service or is refused.

## Where to start

| File                           | What it holds                                                                                                                                                |
| ------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `CLAUDE.md`                    | The always-loaded brief: the one rule, the terms that mislead, the layering, and which document answers which question. Start here if you are an agent.       |
| `CONTEXT.md`                   | The glossary. Read this first — several terms are deliberately opinionated.                                                                                  |
| `DESIGN.md`                    | Types, layering, the state machine, the dependency budget, and what is measured.                                                                             |
| `services/README.md`           | The one service that ships, why a configuration naming none is refused rather than defaulted, and why it names MultiAgency on purpose.                       |
| `docs/PRD.md`                  | What the product is for and what each capability must become. A **target** document: almost nothing in it is built, and a requirement in it is not evidence. |
| `docs/INVARIANTS.md`           | The bar. §A–J extracted from the reference; §K what this implementation earned; §L confinement at the pinned runtime.                                        |
| `docs/ACCOUNT-COORDINATION.md` | The dogfooded operator journey, its stable service binding, and its synthetic acceptance suite.                                                              |
| `docs/experiments/`            | Every measurement, and the transcripts under it. Start at its `README.md`, which says what the arc establishes and what it cannot.                           |
| `docs/FRONTEND.md`             | What a rendered surface is for, who reads it first, and the order the pieces earn. The pages generate; nothing is deployed.                                  |
| `docs/UPGRADE.md`              | Every pin bump, what it was measured against, and the procedure for the next one.                                                                            |
| `docs/adr/`                    | Decisions that are hard to reverse, and why. Indexed in its own `README.md`.                                                                                 |

Everything else is where it looks: `scrap/` and `services/` hold what is compiled into the binary,
`fixtures/` synthetic records, `dev/` the probe stack and proof scripts, `deploy/` the systemd
units, `site/` the generated pages. `MODEL_PIN` and `IRONCLAW_PIN` are the model and runtime of
record. `DESIGN.md` has the crate layering and the rule Cargo enforces.

## The one rule

Run official, unmodified IronClaw. Everything here is configuration, data and code that runs
_around_ it. A change that requires editing IronClaw's source belongs upstream, not here.

Two corollaries worth stating separately:

- **Never add a fallback literal for a pin.** A default is the one value that can silently
  outrank the configuration, and the model pin is a privacy claim, not a version string.
- **The authority is the pinned source**, then upstream releases and issues, then nothing. A blog
  post, a forum answer, or a model's recollection is not a source for runtime behaviour. The
  upstream contract facts in `DESIGN.md` were read out of the pinned revision.

## Relationship to the current IronWorks

`~/agency/ironworks` is the working Python implementation this replaces. This is a
**specification-compatible reimplementation, not a source-compatible port** — see
[ADR 0001](docs/adr/0001-reimplement-ironworks-in-rust.md). Trust guarantees, isolation,
lifecycle, delivery and acceptance behaviour must be preserved; Python structure, storage
schemas, file formats and module names need not be. **Preserved is the obligation, not the
claim** — lifecycle is not built here yet, and `docs/INVARIANTS.md` §F says what that owes.

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
