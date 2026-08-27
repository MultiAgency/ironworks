# Map: A second admitted tenant

Label: `wayfinder:map`
Charted: 2026-09-02

## Destination

**Every gate `docs/PRD.md` §23 names for admitting a second client tenant is cleared and
measured**, so that admitting one is a hand-written registry entry and nothing else.

The act of admitting the real tenant is the **handoff beyond this map**, not a ticket in it: it
needs real credentials and real records, and this tree is synthetic-only by rule (`README.md`,
"Fixtures"). A map whose last step could not be evidenced inside the tree would fail the §1 bar,
which is legibility.

## Notes

**Domain.** `CONTEXT.md` is the vocabulary and it is deliberately opinionated. In this map:
*tenant* (never client/customer/user), *org* (the server-resolved scope key), *slug*, *relay*
(one poll cycle, not a daemon), *confinement* (custody-backed, never "enforced"), and *verdict* is
three-valued — **BLOCKED is never a pass, REVIEW is never FAIL**.

**The one rule.** Official, unmodified IronClaw. No fallback literal for a pin, ever. The
authority is the pinned source, then upstream releases and issues, then nothing.

**Citations.** `path::symbol`, never `path:line`. `cargo run -p ironworks-claims` is the control
and it refuses in-tree line anchors. It was green at charting: 38/38 hold, 0 disagree, tests pass,
clippy and fmt clean (2026-09-02).

**Re-measure rather than repeat.** Before quoting a figure, run the thing or read the file that is
checked. `DESIGN.md` "Status" and `docs/INVARIANTS.md` §I are where the current ones live, and both
carry the date the number was measured on.

**Posture: planning, with `task` tickets where doing unblocks deciding.** This map produces
decisions. `task` tickets exist here only because §23 step 1 and step 4 are execution that a later
*decision* waits on — installing the relay and standing up the synthetic pair. They earn their
place by unblocking a decision, not by delivering the destination.

**Skills.** Grilling tickets: call `grilling` and `domain-modeling`. Research tickets: `research`.
Prototype tickets: `prototype`.

**A negative check needs a positive control.** An empty store passes every cross-org test. Any
isolation check added on this map inherits §B-3's obligation, and §K-8/§F-14 say a harness that can
invent a match *or* invent a difference has the same defect either way.

## Decisions so far

<!-- one line per resolved ticket: gist plus link; the detail lives in the ticket -->

_None yet — charted 2026-09-02, nothing resolved._

## Not yet specified

Fog toward the destination: in scope, not yet sharp enough to ticket.

- **Guidance authoring for tenant two.** Who writes their guidance, how it is validated, and
  whether the operator writes it on their behalf. Hangs on who the tenant is.
- **What channel tenant two arrives on.** Today one `TELEGRAM_BOT_TOKEN` reaches one process
  (`crates/telegram/src/main.rs`). Whether a second tenant means a second bot, a second relay, or
  neither, hangs on the per-tenant loop mechanism.
- **What confinement reconciliation raises to.** §13 says at scale it must run per member on a
  schedule and *raise*, not merely report on demand — but §1 puts alerting out of scope
  deliberately. What "raise" means with no alerting path is unresolved and hangs on the
  confinement decisions.
- **Whether the single-poller availability cap is closed or accepted.** §I records it as a bound
  that caps any service-level promise. Whether this milestone closes it or records it as an
  accepted risk hangs on the per-tenant loop mechanism.
- **§23 step 3's second trigger.** The operator-only binding deferral ends at a second tenant *or*
  at `relationship-intelligence` being relayed into a channel the operator does not solely occupy.
  What the second trigger obligates, separately from the first, is not yet sharp.

## Out of scope

Beyond the destination. These do not graduate; they return only if the destination is redrawn.

- **The answer-quality claim** (§23 step 2, `docs/FRONTEND.md`). §23 says outright it is a
  different milestone reached on its own schedule, gating a claim rather than a tenant.
- **Self-serve** — provisioning execution (§9), the console (§16), guided authoring. §1: not a
  funded milestone. §23 puts them after this one.
- **Registry refusal redesign** (§23 step 8). Explicitly can wait until tenant counts make a
  whole-set outage unacceptable rather than merely inelegant.
- **Off-box backup and rehearsed restore** (§I). A real gap; not forced by a second tenant.
- **The unknown-keys asymmetry** between service definitions and the tenant registry (§I). A real
  gap; not forced by a second tenant.
- **Admitting the real tenant.** The handoff on the far side of the destination.
