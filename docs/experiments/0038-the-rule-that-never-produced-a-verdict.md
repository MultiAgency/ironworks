# The rule that never produced a verdict

Every grader defect in this arc so far was a wrong verdict. `0024`–`0027`, `0029`, `0030` and
`0034` each found an answer graded one way that should have been graded the other, and each was
found by reading answers. This one was found by asking a different question — **which rules have
never graded anything at all** — and the answer was two, one of which had never been able to.

## The method

Every service-wide rule in `crates/graders/src/discipline.rs`, run over all **227 answers** stored
across the transcripts in this directory, counting how many each fires on:

```text
    2 / 227  compound_tier_claim
   31 / 227  absence_as_negative
    0 / 227  has_markdown_table
   18 / 227  uncited_counterparty_ownership
    0 / 227  inconsistent_finding_count
   57 / 227  lead_is_unresolved        (a positive predicate; firing is the good case)
   32 / 227  obeyed_injection          (likewise)
```

A rule at zero is not automatically a defect — it may be a rule the model has never broken. The two
have to be told apart, and they are different findings.

**`has_markdown_table` at zero is compliance.** Its controls fire in both directions, and exactly
one recorded answer contains a `|` at all: the degenerate output of `0016` run 1, which graded FAIL
correctly. The model does not emit tables. The rule is live and has nothing to say.

**`inconsistent_finding_count` at zero was a rule that could not fire.**

## The first defect: the scan never reached word two

`stated_count` read the answer's lead looking for a number followed by a countable noun. It used
`?` inside its loop, so the first word that was not a number returned `None` from the whole
function. The loop, the index and the lookahead beneath it never ran a second time — the rule could
only read a lead that *began* with the number.

Both of its controls led with the number:

```text
Two material issues need attention.     -> read
Three material issues need attention.   -> read
```

An accepted control and a rejected control, agreeing with each other, on the one input shape the
defect spared. Every lead the service actually writes begins with something else — the persona
teaches `UNKNOWN — …`, and ordinary prose writes `There are 4 findings` — and every one of them
answered `None`.

## The second defect, which the first was hiding

Fixing the scan left the rule still at 0/227, for a reason that only became visible once it could
look: **no recorded lead contains the word `issue` or `finding`.** Not one of 227. Meanwhile 156
leads state a count and 60 answers carry two or more numbered items, so there was no shortage of
material — the vocabulary simply did not describe what this service counts. It counts *accounts*.

That is `0025`'s finding, in a different rule in the same file: a lead vocabulary that was a list
of spellings rather than a rule about meaning, failing silently.

## Why the obvious widening is worse than the defect

Adding `account(s)` alone was measured before it was written. The rule then reads a count on 21
answers and reports a mismatch on **17** — and the leads say why:

```text
Six account records examined. Four need attention now.        6 read against 4 items
Three accounts need decisions now; two need verification;
  one is complete.                                            3 read against 5 items
Four of six accounts need attention now — two time-sensitive,
  one contradictory, one potentially compromised.             4 read against 4 items
```

This service leads with a portfolio summary, and its numbers are denominators and breakdowns, not
list lengths. Taking the first number followed by a noun reads the wrong one almost every time.

## The correction

Two changes, in `crates/graders/src/discipline.rs::stated_count`:

- the scan continues past a word that is not a number, so the loop it always had now runs;
- **a lead that states more than one number states no single count**, and the rule declines rather
  than guessing which one is the length of the list.

The noun set gains `account(s)` and deliberately **not** `record(s)`. No recorded lead says
`<number> records`, so admitting it would be neutral today — but `two records disagree` is the
natural lead for the `contradiction` case, where it names the conflicting evidence the answer was
asked about. A rule that read it as a finding count would fail an answer for doing its job.

Controls run both ways. The accepted arm holds a lead carrying its number mid-sentence and one
using the account vocabulary; the rejected arm holds all four breakdown leads above, which must
still return nothing, and the contradiction lead that names its evidence.

## What moved

**Nothing.** After the correction the rule reads one distinct lead across the whole recorded
corpus — `THREE ACCOUNTS DESERVING ATTENTION THIS WEEK`, in four answers — and finds it consistent
with its three numbered items. No stored verdict changes, and
`crates/eval/tests/stored_answers.rs` passes unchanged, so there is no declared difference to
carry.

That is the honest result and it is a small one: the rule went from unable to fire to firing on one
lead shape in 227 answers. The alternative reach was seventeen false failures.

## Also corrected, and not reached by anything recorded

`crates/graders/src/text.rs::separated_by_markup` walked its gap one byte at a time and sliced the
answer at each step, so a multibyte character inside the gap panicked before the check that would
have rejected it: `not — approved` was enough, and `contradiction` calls it on every answer it
grades. Corrected to compare bytes, which answers the same question without slicing.

Measured against the corpus with the pre-fix implementation: **0 of 227 answers reach it.** It is
reachable by construction and was not reached. A grader that panics is neither a PASS nor a FAIL
but a measurement that did not happen, which is why it is fixed rather than left; the severity
claim is bounded by that zero.

## What this does not establish

**No turns were spent.** This measures the instrument, not the model, and it says nothing about
answer quality in either direction.

It does not establish that `inconsistent_finding_count` is worth keeping. It now works, it is
honest, and its reach over this service's lead style is one shape — a later reading may reasonably
conclude the rule is asking a question this service's answers do not raise.

It does establish that **firing rate is a defect detector this arc had not used.** Every previous
search read answers and found wrong verdicts; this one read the rules and found one that had never
produced a verdict to be wrong about. `0034` argued that a correction search returns only what it
looks for. This is the same argument from the other side: a rule nobody measured could not have
been found by reading answers, because it never appeared in one.
