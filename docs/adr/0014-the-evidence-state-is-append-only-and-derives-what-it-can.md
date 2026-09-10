---
status: accepted
---

# 0014 — The evidence state is append-only, and derives everything it can

## Context

`docs/adr/0013-one-evidence-vocabulary-with-two-existing-projections.md` decided what an
observation *says* and deliberately left open where it lives. Three decisions about storage are
hard to reverse once a store exists and rows are in it, so they are recorded together here.

The plan this came from said "reuse the `crates/proposals` buffer shape; do not invent storage."
The first draft of `crates/evidence/src/state.rs` ignored that and called `rusqlite::Connection::open`
directly with its own version settlement — which would have shipped a **world-readable file holding
tenant record ids**, because owner-only permissions are one of the four things `crates/store` exists
to own. That mistake is why the first decision below is stated as a rule rather than as a habit.

## Decisions

**1. It goes through `ironworks-store`, like every other SQLite file here.** `crates/store` owns
owner-only permissions, WAL, `synchronous = FULL`, schema creation and version settlement, and it
owns them *once*. A store that opens its own connection is a second mechanism for a solved problem
and will differ from the first on whichever of the four its author forgot. `EvidenceError` maps
every `OpenFailure` variant by name rather than by message, so a caller can tell a wrong-version
store from an unreadable one without parsing prose.

**2. Append-only, enforced by the API rather than by convention.** There is no `update` and no
`delete`. A claim observed twice is two rows, and the later one does not overwrite the earlier:
both stand, each against the candidate it was made about. That is the only way a reader can see
that a finding *moved*, which is exactly what a planner needs and what an in-place update destroys.
`seq` is the primary key for the same reason — a key over `(loop, claim)` would have made the
second observation silently replace the first.

**3. Anything derivable from evidence is derived, never authored.** arXiv 2609.01481 §3.4.1 has its
planner *write* the list of functionality to preserve, which makes preservation something a planner
can forget — invisibly, because the increment simply breaks something and the next iteration reads
the breakage as new work. Here `Increment::preserving` reads that set from the evidence state, so a
document cannot omit a claim standing as upheld against the candidate it plans against. A planner
chooses **scope**; it does not choose what to protect.

The same rule explains what `D_t` does *not* do: it renders to Markdown and there is no parser
back. The next role reads it as text, and the machine-readable half needs no round trip because it
is rebuilt from the state. What the file carries that nothing else can reconstruct is the scope a
planner chose — so that is all it has to carry.

## Consequences

**The four sets are the interface, not a convenience.** `EvidenceState::standing` returns
`preserve` / `repair` / `unresolved` / `unmeasured` separately, because the actions differ: keep
it, fix it, have a person read it, or go and measure it. Merging `repair` with `unmeasured` is the
paper's `verified ∪ gap`, and it sends an iteration to repair something nobody looked at.

**The controls can fail, and that was checked rather than assumed.** Three tests in
`crates/evidence/src/state.rs` are the loop controls Phase 4 asked for. Implementing the two-valued
collapse makes exactly one of them fail — `control_a_blocked_claim_is_never_planned_as_repair` —
while the other twenty-four pass. A control that cannot fail is not measuring, and this one was run
against the defect it exists for.

**`upheld_for` delegates to `standing`.** Both computed "the latest finding per claim" at first,
which is two mechanisms for one rule; they would have drifted on the one question that matters,
which finding wins when a claim was observed twice.

**What this does not decide.** Who writes observations, and under what authority. Nothing here
grants a model role write access to an evidence state, and Phase 5's confined roles are the
decision that would. An evidence state written by something that also plans against it is a
different design and needs its own record.
