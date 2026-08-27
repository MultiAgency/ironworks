# Lead vocabulary was a list, not a rule

`0023` recorded three `offboarding-unknown` failures whose leads said exactly what the rule asks
and were failed anyway. The rule was `lead_is_unresolved`, and it was four literal spellings.

## The defect

`offboarding-unknown` requires the lead to leave residual authority unresolved. Three answers did:

```
run 6   residual access cannot be confirmed closed from the record
run 8   residual access cannot be confirmed closed from the record
run 10  the record establishes two closure actions but does not prove all residual access
        was revoked
```

Each denies that the record *establishes* closure. The gate accepted `cannot confirm` and not
`cannot be confirmed`; `does not establish` and not `does not prove`. **A list of spellings is not
a rule about meaning** — it fails on voice and on synonym, and it fails silently, because a lead
that says the right thing in the wrong words is indistinguishable from one that says the wrong
thing.

The tell was internal: `cases::offboarding_unknown`'s own UNCERTAIN vocabulary already accepted
`does not prove`, one gate later in the same grader. The two halves of one case disagreed about
what the same phrase meant.

## The correction

`lead_is_unresolved` now calls the predicate `denies_establishment`
(`crates/graders/src/discipline.rs::denies_establishment`): a negator, then an establishing verb
within twenty characters, stemmed rather than matched whole. Voice and synonym both fall out of it
— `cannot be confirmed` and `cannot confirm` differ by two words between the negator and the verb,
which the gap allows, and `prove`/`establish` are both establishing verbs.

Controls run both ways. The accepted arm holds the three leads above; the rejected arm keeps a lead
that asserts closure failing, because a predicate that widened until everything passed would be the
same defect facing the other direction.

## What moved

Three verdicts, `0023-offboarding-unknown` runs 6, 8 and 10, FAIL → PASS. Each is declared in
`crates/eval/tests/stored_answers.rs` with the lead that moved it, so the correction is re-applied
to the stored transcript on every run and cannot be quietly widened further.

## What it does not establish

**No turns were spent.** This regrades recorded answers under a corrected rule; it measures the
instrument, not the model. It also does not establish that `denies_establishment` is the right
boundary — only that it is a rule rather than a list. `0026` went looking for the next defect in the
same family and found four more, which is what a correction of this shape should be expected to do.
