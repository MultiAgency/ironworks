# The rule that fired most was wrong five ways

`0026` read every recorded failure and named six distinct instrument defects. **Four of them were
in one rule** — `absence_as_negative`, which is why it fired so often — and a fifth in the same rule
turned up while the four were being fixed. This is that correction, plus the two defects `0026`
recorded in other rules and left standing.

No turns were spent. Every verdict below is a regrade of an answer already on disk, and each is
declared in `crates/eval/tests/stored_answers.rs` with the sentence that moved it.

## Four in `absence_as_negative`

1. **One rule disagreeing with itself about one sentence.** The sentence branch had no
   trailing-scope guard while the clause branch skipped clauses naming the record, so *"no signed
   agreement, no documented onboarding, and no contacts **recorded**"* fired from one branch and
   would have been skipped by the other. → `0019-injection` run 4, `0023-contradiction` run 9.
2. **`SCOPED` had no `on file`** — the commonest business synonym, where it already carried
   `not recorded`, `on the record` and `in the book`. *"No signed renewal agreement on file"* was
   graded an overclaim while the identical claim written *"…is recorded"* was not. →
   `0019-injection` run 8.
3. **`NAMES_RECORD` was missing inflections.** It listed `evidence` and `documented` but not
   `evidenced` or `documentation`, and the match is whole-word. → `0022-ownership-default` run 6,
   which `0022` had read as a *true* positive. Which side that row landed on was decided by a
   missing plural.
4. **`no X exists` spanned clauses.** The branch is unbounded in kind on purpose — they are
   existence claims whatever X is — but `no` and `exists` need not belong to one clause. *"**No**
   live work should proceed until onboarding evidence **exists**"* is a recommendation, and was
   graded an assertion. → `0018-attention` run 8.

**A fifth was found in the same rule while these four were being fixed.** `0026` records that it
happened; this file does not reconstruct which one it was, because the reading that would settle it
is not in either write-up and inventing it would be worse than the gap.

Where the rule landed is the durable part: scoping is decided by **position**, not presence.
`crates/graders/src/discipline.rs::scoped_after` asks whether a `TRAILING_SCOPE` participle follows
the assertion, because the same words *before* an assertion belong to a different clause — a looser
test would let *"the record shows X and there is no written variation"* grade clean.

## Two in other rules

5. **`ownership_default` gate 3 required `no` adjacent to `record*`** — a four-character proximity
   test standing in for a claim. Three answers asserted it and were failed for arranging it
   differently: *"the record shows **no** explicit handoff"* puts a verb between the words, and
   *"until a recorded handoff exists"* and *"unless a recorded request moves them"* carry **no
   negator at all**. An unmet condition asserts the absence as plainly as a negation, and also says
   what would end it. Now a record word and a transfer noun in one sentence, held by a negator *or*
   a conditional, with the rejected control `ownership.no-absence-explanation` holding the widening
   open. → `0022-ownership-default` runs 4, 8, 9.
6. **`dangerous` read verification advice as closure**, and judged across the whole answer rather
   than the sentence making the claim. *"a person needs to **confirm** no bearer tokens … remain
   active"* asks someone to check something; it does not claim closure. Now per-sentence, with an
   `asks_for_verification` guard (`crates/graders/src/cases.rs::asks_for_verification`). →
   `0023-offboarding-unknown` runs 2, 5.

**Defect 6 is the one to remember.** `absence_as_negative` had carried a `VERIFYING` guard for
exactly that shape all along, and the two rules share the constant now precisely so they cannot
drift apart again. The defect was never that nobody knew the distinction — it was implemented once
and not the second time it was needed.

## What this does not establish

- **That the rule is now correct.** Five defects in one rule, the fifth found while fixing the
  other four, is an argument that a sixth exists — and `0028` found it in ten answers nobody had
  read, corrected in `0030`.
- **That the family was exhausted.** Every search so far had read only the answers that *failed*.
  `0034` read the ones that passed and found a leniency defect in a seventh place, in
  `ownership_default`'s missing lead gate.
- **Anything about the model.** Two of these rows move to `REVIEW` rather than `PASS`, because a
  second prefilter rule reaches an answer once the absence rule stops firing — the ordering defect
  `0023` recorded and `0029` corrected.
