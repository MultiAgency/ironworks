# A REVIEW was standing in front of a decided verdict

`0023` recorded a `contradiction` answer as REVIEW and noted that the case grader had never run.
This is that defect: the service-wide prefilter **returned** on its two undecidable rules, so an
answer a case grader could have failed on its merits was reported as one nothing could judge.

## The defect

`evidence_disciplined` (`crates/graders/src/discipline.rs::evidence_disciplined`) runs three
fail-fast rules, then two that produce REVIEW — `uncited_counterparty_ownership` and
`inconsistent_finding_count` — then the case grader. The two REVIEW rules returned immediately.

So the order of evaluation decided the verdict. Three recorded answers were masked:

| transcript | run | reported | the case grader's own verdict |
|---|---|---|---|
| `0023-contradiction` | 7 | REVIEW | **FAIL** — a contradictory yes-or-no question must lead with an unresolved verdict |
| `0022-ownership-default` | 1 | REVIEW | **FAIL** — moves ownership outward without explaining that no recorded request does so |
| `0022-ownership-default` | 2 | REVIEW | **FAIL** — same shape |

**REVIEW is never a synonym for FAIL** (`CONTEXT.md`), and that is exactly why this matters: a
REVIEW says *read this answer, the instrument cannot decide*. Three answers the instrument **could**
decide were filed as three it could not, and the headline `contradiction` case was one of them.

## The correction

The prefilter now **holds** its review instead of returning it, and ranks:

```rust
match held {
    // A measured defect outranks an undecidable one.
    Some(_) if graded.verdict == Verdict::Fail => graded,
    Some(review) => review,
    None => graded,
}
```

That is the ordering `crates/lifecycle` already uses, where `Fail` outranks `Blocked`. It can only
sharpen a verdict, never soften one: a *passing* case grader still leaves the review standing, so
an answer with an uncited ownership claim is still reported for reading.

## What moved

Three verdicts, REVIEW → FAIL. All three are declared in `crates/eval/tests/stored_answers.rs`.

**This is the one correction in the arc that moved verdicts downward**, and it is worth naming for
that reason alone: at the time, every other correction had moved a verdict up, and the ratio was
being read as evidence that the instrument was being tuned toward a number. It was not — `0034`
later established that every *search* had been one-directional — but this row was already the
counterexample and nobody counted it, because it changed a REVIEW rather than a FAIL.

## What it does not establish

**No turns were spent.** And note what `0022` run 2 does *not* rest on: `0026` records the
uncited-ownership review here as itself a false positive — an explicit UNKNOWN about which side is
waiting is not an ownership move. Correcting that false positive will not move this row, because
the case grader fails the answer independently. Two defects on one answer, and only one of them is
load-bearing for its verdict.
