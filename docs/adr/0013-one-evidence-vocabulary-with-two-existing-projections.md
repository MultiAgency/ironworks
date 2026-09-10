---
status: accepted
---

# 0013 — One evidence vocabulary, and the existing verdicts are projections of it

## Context

An evidence state (`E_t`, from arXiv 2609.01481 §3.3) has to carry the status of every claim it
knows about, across iterations, in a form the next planning step can read. This repository already
had **four** ways of saying what an observation established, and they disagree about what the
non-pass cases are:

| where | cases | what its third case means |
|---|---|---|
| `ironworks_graders::Verdict` | `Pass`, `Fail`, `Review` | the instrument could not decide |
| `ironworks_lifecycle::inspect::Verdict` | `Pass`, `Fail`, `Blocked` | the measurement did not happen |
| `ironworks_claims::Verdict` | `Pass`, `Fail`, `Blocked` | the check could not be performed |
| `ironworks_eval`'s driver | a verdict, or `Blocked(String)` | the turn produced no answer |

*Corrected 2026-09-10: this said **three** and listed the first, second and fourth rows. The
control's own verdict type was missed while writing an ADR whose subject is not adding another one
— which is the argument for the decision below rather than against it.*

A fifth vocabulary was the obvious mistake and the one worth deciding against explicitly. But
picking one of the four was also wrong: `graders` has no way to say *nothing was observed*, and
`inspect` has no way to say *observed and undecidable*. Each is complete for its own question and
neither is complete for this one.

The paper takes the opposite path — §A.4 partitions its evidence bundle into `verified ∪ gap`,
disjoint and exhaustive, with "claims without sufficient evidence" placed in `gap` beside observed
failures. That is the collapse `docs/experiments/0050-blocked-was-being-read-as-fail.md` measured
costing seven of eleven blocking causes a wrong verdict, and that §F-14 exists to forbid.

## Decision

**`ironworks_domain::Finding` is the one vocabulary an evidence state uses**, and it is the *join*
of two independent questions rather than a fourth opinion about one scale:

1. was the observation made at all — `Unobserved` versus the rest;
2. if it was, what did it establish — `Upheld`, `Refuted`, `Indeterminate`.

The existing verdict types are **projections** of it and neither changes. `graders::Verdict` maps
onto the three observed cases and can never produce `Unobserved`, which is asserted rather than
assumed (`ironworks-eval`'s `no_verdict_can_produce_unobserved`). `inspect::Verdict` is the
flattening that a reconciliation needs, and its lack of an `Indeterminate` is correct for a
question the world answers determinately or not at all.

It lives in `crates/domain` because both an evidence store and the eval driver must see it. The
projection from a grader's verdict lives in `crates/eval`, because that is the **only** crate that
can see both: `ironworks-graders` is a leaf with no dependencies at all, and `ironworks-domain`
cannot reach it. That constraint chose the location, not preference.

## Consequences

**The two-valued partition is now demonstrably lossy rather than arguably so.**
`finding::tests::the_two_valued_partition_cannot_be_inverted` models the paper's bucket as a
function and shows it is not injective: three distinct states arrive at the next iteration as one
instruction. A `gap` that is really `Unobserved` sends the next loop to repair something that may
already work, and that repair's own evidence is then read as progress.

**The eval driver counts on the type instead of on strings.** Its tally used to match on
`"PASS" | "FAIL" | "REVIEW" | _ => blocked`, so any verdict spelling nobody had counted was
silently tallied as an unmeasured run. The match is now exhaustive over `Finding` and the compiler
refuses a new case that nobody has counted. Transcripts gain a `finding` field beside the existing
`verdict` string; the string stays, because dated transcripts are read by tallies that already
know it.

**What this does not decide.** Nothing here specifies how `E_t` is stored, what a claim id is, or
what `D_t` looks like — those need the storage decision that `crates/proposals`'s shape suggests
and this ADR deliberately does not pre-empt. It decides only the vocabulary, which is the part that
would have been expensive to change after a store existed. *Those questions were decided the same
day in `docs/adr/0014-the-evidence-state-is-append-only-and-derives-what-it-can.md`; this record
keeps its own scope rather than absorbing them.*

**A fifth verdict enum is now a defect rather than a choice.** If a future case needs saying, it is
added to `Finding` and projected, never modelled again alongside it.
